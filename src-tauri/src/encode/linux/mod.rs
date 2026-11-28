//! Codificador H.264 do Linux: OpenH264, no processador.
//!
//! O OpenH264 codifica na hora, dentro da chamada. Para cumprir o contrato
//! (`encode` só entrega o frame e volta), ele roda numa thread própria: o
//! frame é copiado para I420 e entra numa fila curta; o resultado sai por
//! [`OnEncoded`] dessa thread. Se a fila já está cheia, o codificador está
//! atrasado e o frame novo é descartado (o próximo da captura chega logo).
//!
//! Sai H.264 Constrained Baseline, com SPS e PPS antes de cada quadro-chave,
//! que é o que o WebRTC do app anuncia.

use super::{EncodedFrame, EncoderConfig, OnEncoded, VideoEncoder};
use crate::capture::{
    contracts::BoxError,
    packet::{PixelFormat, VideoPacket},
};
use openh264::{
    encoder::{
        BitRate, Complexity, Encoder, EncoderConfig as H264Config, FrameRate, FrameType, IntraFramePeriod,
        RateControlMode, UsageType, VuiConfig,
    },
    formats::YUVSlices,
    OpenH264API, Timestamp,
};
use openh264_sys2::{ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SPATIAL_LAYER_0, SPATIAL_LAYER_ALL, TagBitrateInfo};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Quantos frames podem estar esperando ou sendo codificados. Acima disso o
/// codificador está atrasado, e o frame novo é descartado.
const MAX_INSIDE: u32 = 2;

/// Um frame a caminho do codificador, já em I420.
struct Job {
    i420: Vec<u8>,
    timestamp: Duration,
    entered: Instant,
}

#[derive(Default)]
struct Timings {
    frames: u32,
    out: u32,
    dropped: u32,
    held_back: u32,
    copy: Duration,
    latency: Duration,
}

/// O que a thread do codificador e quem chama dividem.
struct Shared {
    /// Frames na fila ou sendo codificados.
    inside: AtomicU32,
    /// O bitrate pretendido (a thread aplica antes do próximo frame).
    bitrate: AtomicU32,
    /// Quantas vezes o OpenH264 recusou um bitrate novo.
    refused: AtomicU32,
    /// Alguém pediu um quadro-chave: o próximo frame codificado é um.
    keyframe: AtomicBool,
    timings: Mutex<Timings>,
    /// Buffers I420 já usados, para não alocar um por frame.
    spare: Mutex<Vec<Vec<u8>>>,
}

pub struct OpenH264Encoder {
    config: EncoderConfig,
    shared: Arc<Shared>,
    queue: Option<mpsc::SyncSender<Job>>,
    worker: Option<JoinHandle<()>>,
}

/// Copia os pixels de um frame para I420 (planos Y, U e V seguidos), que é o
/// que o OpenH264 aceita. NV12 tem o U e o V intercalados num plano só.
fn to_i420(frame: &VideoPacket, out: &mut Vec<u8>) {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let px = frame.pixels();
    out.clear();
    match frame.format() {
        PixelFormat::I420 => out.extend_from_slice(px),
        PixelFormat::Nv12 => {
            let (y, uv) = px.split_at(w * h);
            out.reserve(px.len());
            out.extend_from_slice(y);
            out.extend(uv.iter().step_by(2));
            out.extend(uv.iter().skip(1).step_by(2));
        }
    }
}

/// O bitrate que o OpenH264 aceita (ele conta em `int`).
fn bitrate_for(bps: u32) -> i32 {
    bps.clamp(50_000, i32::MAX as u32) as i32
}

/// Muda o bitrate sem reiniciar o codificador e devolve quantas das mudanças
/// o OpenH264 recusou.
///
/// O OpenH264 tem dois tetos, o da camada e o geral, e recusa um alvo acima
/// do teto da camada, que nasce igual ao bitrate com que o codificador abriu.
/// Por isso os tetos sobem antes do alvo e descem depois dele.
fn apply_bitrate(encoder: &mut Encoder, from: u32, to: u32) -> u32 {
    let set = |encoder: &mut Encoder, option, layer| {
        let mut info = TagBitrateInfo { iLayer: layer, iBitrate: bitrate_for(to) };
        // SAFETY: o codificador já foi iniciado (só é chamado depois do primeiro
        // frame) e as duas opções recebem um `SBitrateInfo`, que é esta
        // estrutura; ela vive até a chamada voltar.
        unsafe { encoder.raw_api().set_option(option, (&raw mut info).cast::<c_void>()) }
    };
    // há uma camada só (`SPATIAL_LAYER_0`); `SPATIAL_LAYER_ALL` é o total
    let ceilings = [(ENCODER_OPTION_MAX_BITRATE, SPATIAL_LAYER_0), (ENCODER_OPTION_MAX_BITRATE, SPATIAL_LAYER_ALL)];
    let target = [(ENCODER_OPTION_BITRATE, SPATIAL_LAYER_ALL)];
    let order = if to > from { [ceilings[0], ceilings[1], target[0]] } else { [target[0], ceilings[0], ceilings[1]] };
    let mut refused = 0;
    for (option, layer) in order {
        let status = set(encoder, option, layer);
        if status != 0 {
            refused += 1;
            crate::log_warn!("sharing", "OpenH264 refused the new bitrate of {} kbps (error {status})", to / 1000);
        }
    }
    refused
}

impl OpenH264Encoder {
    pub fn open(config: EncoderConfig, on_encoded: OnEncoded) -> Result<Self, BoxError> {
        if config.width % 2 != 0 || config.height % 2 != 0 {
            return Err(format!("the encoder needs even sizes, got {}x{}", config.width, config.height).into());
        }
        let h264 = H264Config::new()
            .bitrate(BitRate::from_bps(bitrate_for(config.bitrate) as u32))
            .max_frame_rate(FrameRate::from_hz(config.fps.max(1) as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            // tela: muita área parada e texto, que este modo trata melhor
            .usage_type(UsageType::ScreenContentRealTime)
            .complexity(Complexity::Low)
            // quadro-chave só quando alguém pede (quem entra, quem perdeu um frame)
            .intra_frame_period(IntraFramePeriod::from_num_frames(0))
            // as cores da captura (BT.709), para quem decodifica não adivinhar
            .vui(VuiConfig::bt709());
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), h264)
            .map_err(|e| format!("OpenH264 did not start: {e}"))?;

        let shared = Arc::new(Shared {
            inside: AtomicU32::new(0),
            bitrate: AtomicU32::new(config.bitrate),
            refused: AtomicU32::new(0),
            keyframe: AtomicBool::new(false),
            timings: Mutex::default(),
            spare: Mutex::default(),
        });
        let (queue, jobs) = mpsc::sync_channel::<Job>(MAX_INSIDE as usize);
        let state = shared.clone();
        let (w, h) = (config.width as usize, config.height as usize);
        let worker = std::thread::Builder::new()
            .name("openh264".into())
            .spawn(move || {
                let mut applied = config.bitrate;
                let mut started = false;
                while let Ok(job) = jobs.recv() {
                    let wanted = state.bitrate.load(Ordering::Relaxed);
                    if started && wanted != applied {
                        let refused = apply_bitrate(&mut encoder, applied, wanted);
                        state.refused.fetch_add(refused, Ordering::Relaxed);
                        applied = wanted;
                    }
                    if state.keyframe.swap(false, Ordering::Relaxed) && started {
                        // o primeiro frame já é um quadro-chave
                        encoder.force_intra_frame();
                    }
                    let (y, chroma) = job.i420.split_at(w * h);
                    let (u, v) = chroma.split_at(w * h / 4);
                    let source = YUVSlices::new((y, u, v), (w, h), (w, w / 2, w / 2));
                    let result = encoder
                        .encode_at(&source, Timestamp::from_millis(job.timestamp.as_millis() as u64))
                        .map(|bits| (bits.frame_type(), bits.to_vec()));
                    started = true;
                    state.inside.fetch_sub(1, Ordering::Relaxed);
                    let mut timings = state.timings.lock().unwrap();
                    match result {
                        Ok((kind, data)) if !data.is_empty() && !matches!(kind, FrameType::Skip | FrameType::Invalid) => {
                            timings.out += 1;
                            timings.latency += job.entered.elapsed();
                            drop(timings);
                            on_encoded(EncodedFrame { keyframe: super::has_idr(&data), data, timestamp: job.timestamp });
                        }
                        // o controle de banda deixou este frame de fora
                        Ok(_) => timings.held_back += 1,
                        Err(e) => {
                            drop(timings);
                            crate::log_warn!("sharing", "OpenH264 could not encode a frame: {e}");
                        }
                    }
                    state.spare.lock().unwrap().push(job.i420);
                }
            })
            .map_err(|e| format!("could not start the encoder thread: {e}"))?;

        Ok(Self { config, shared, queue: Some(queue), worker: Some(worker) })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn describe(&self) -> String {
        "OpenH264 (processor)".into()
    }

    fn hardware(&self) -> bool {
        false
    }

    fn encode(&mut self, frame: &VideoPacket, timestamp: Duration, keyframe: bool) -> Result<(), BoxError> {
        if (frame.width() as u32, frame.height() as u32) != (self.config.width, self.config.height) {
            return Err(format!(
                "frame is {}x{} but the encoder was opened for {}x{}",
                frame.width(),
                frame.height(),
                self.config.width,
                self.config.height
            )
            .into());
        }
        // o pedido fica guardado: mesmo que este frame seja descartado, o
        // próximo que for codificado sai como quadro-chave
        if keyframe {
            self.shared.keyframe.store(true, Ordering::Relaxed);
        }
        self.shared.timings.lock().unwrap().frames += 1;
        if self.shared.inside.load(Ordering::Relaxed) >= MAX_INSIDE {
            self.shared.timings.lock().unwrap().dropped += 1;
            return Ok(());
        }
        let entered = Instant::now();
        let mut i420 = self.shared.spare.lock().unwrap().pop().unwrap_or_default();
        to_i420(frame, &mut i420);
        self.shared.timings.lock().unwrap().copy += entered.elapsed();

        self.shared.inside.fetch_add(1, Ordering::Relaxed);
        let queue = self.queue.as_ref().ok_or("the encoder is closed")?;
        if queue.try_send(Job { i420, timestamp, entered }).is_err() {
            self.shared.inside.fetch_sub(1, Ordering::Relaxed);
            return Err("the encoder thread stopped".into());
        }
        Ok(())
    }

    fn set_bitrate(&mut self, bps: u32) {
        self.config.bitrate = bps;
        self.shared.bitrate.store(bps, Ordering::Relaxed);
    }

    fn take_timings(&mut self) -> Option<String> {
        let t = std::mem::take(&mut *self.shared.timings.lock().unwrap());
        let per = |d: Duration, n: u32| d.as_secs_f64() * 1000.0 / n.max(1) as f64;
        Some(format!(
            "{} frames out ({} skipped because the encoder was still busy, {} left out by the encoder to keep \
             the bitrate), copy to the encoder {:.1} ms, from frame in to frame out {:.1} ms",
            t.out,
            t.dropped,
            t.held_back,
            per(t.copy, t.frames),
            per(t.latency, t.out)
        ))
    }
}

impl Drop for OpenH264Encoder {
    fn drop(&mut self) {
        // fecha a fila e espera a thread codificar o que ainda está nela
        self.queue = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{has_idr, tests::pattern};

    type Out = Arc<Mutex<Vec<EncodedFrame>>>;

    fn open(width: u32, height: u32, bitrate: u32) -> (OpenH264Encoder, Out) {
        let out: Out = Arc::default();
        let sink = out.clone();
        let config = EncoderConfig { width, height, fps: 30, bitrate, software_only: true };
        let encoder = OpenH264Encoder::open(config, Arc::new(move |f| sink.lock().unwrap().push(f))).unwrap();
        (encoder, out)
    }

    /// Entrega um frame e espera ele sair, para o teste não depender do ritmo.
    fn encode(encoder: &mut OpenH264Encoder, frame: &VideoPacket, n: u32, keyframe: bool) {
        encoder.encode(frame, Duration::from_millis(n as u64 * 33), keyframe).unwrap();
        while encoder.shared.inside.load(Ordering::Relaxed) > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// As barras que andam, com um chuvisco leve que muda a cada frame: dá
    /// trabalho ao codificador sem ser impossível de comprimir, como um vídeo.
    fn busy(w: u32, h: u32, n: u32) -> VideoPacket {
        let mut pkt = pattern(w, h, n);
        let mut seed = 0x9e37_79b9u32.wrapping_mul(n + 1);
        for px in &mut pkt.pixels_mut()[..(w * h) as usize] {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *px = px.wrapping_add((seed >> 28) as u8);
        }
        pkt
    }

    #[test]
    fn nv12_and_i420_reach_the_encoder_as_the_same_planes() {
        let mut nv12 = VideoPacket::new(PixelFormat::Nv12, 4, 2);
        nv12.pixels_mut().copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 50, 90, 51, 91]);
        let mut i420 = VideoPacket::new(PixelFormat::I420, 4, 2);
        i420.pixels_mut().copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 50, 51, 90, 91]);
        let (mut a, mut b) = (vec![9; 3], Vec::new());
        to_i420(&nv12, &mut a);
        to_i420(&i420, &mut b);
        assert_eq!(a, [1, 2, 3, 4, 5, 6, 7, 8, 50, 51, 90, 91]);
        assert_eq!(a, b);
    }

    #[test]
    fn starts_with_a_keyframe_and_gives_another_when_asked() {
        let (mut encoder, out) = open(320, 180, 500_000);
        for n in 0..20 {
            encode(&mut encoder, &pattern(320, 180, n), n, n == 10);
        }
        drop(encoder);
        let out = out.lock().unwrap();
        assert_eq!(out.len(), 20, "every frame comes out");
        let keyframes: Vec<usize> = out.iter().enumerate().filter(|(_, f)| f.keyframe).map(|(i, _)| i).collect();
        assert_eq!(keyframes, [0, 10], "a keyframe to start, and the one asked for");
        for f in out.iter().filter(|f| f.keyframe) {
            assert!(f.data.starts_with(&[0, 0, 0, 1]), "Annex B");
            assert_eq!(f.data[4] & 0x1f, 7, "a keyframe starts with the SPS, so a decoder can start from it");
            assert_eq!(f.data[5], 66, "Baseline profile");
            assert!(has_idr(&f.data));
        }
        assert!(out.windows(2).all(|p| p[0].timestamp < p[1].timestamp), "in order, with the time each frame came in");
    }

    #[test]
    fn a_keyframe_asked_on_a_dropped_frame_comes_with_the_next_one() {
        let (mut encoder, out) = open(320, 180, 500_000);
        encode(&mut encoder, &pattern(320, 180, 0), 0, false);
        // faz de conta que o codificador está ocupado: este frame é descartado
        encoder.shared.inside.store(MAX_INSIDE, Ordering::Relaxed);
        encoder.encode(&pattern(320, 180, 1), Duration::from_millis(33), true).unwrap();
        encoder.shared.inside.store(0, Ordering::Relaxed);
        encode(&mut encoder, &pattern(320, 180, 2), 2, false);
        let timings = encoder.take_timings().unwrap();
        drop(encoder);
        let out = out.lock().unwrap();
        assert_eq!(out.len(), 2, "the frame that found the encoder busy was dropped");
        assert!(out[1].keyframe, "the keyframe that was asked for still came");
        assert!(timings.contains("1 skipped because the encoder was still busy"), "{timings}");
    }

    #[test]
    fn a_lower_bitrate_makes_smaller_frames_without_a_restart() {
        let (mut encoder, out) = open(320, 180, 2_000_000);
        let mut n = 0;
        // os bytes que saem de 30 frames, contando só a segunda metade (depois
        // de o controle de banda se ajustar), e se algum saiu como quadro-chave
        let mut run = |encoder: &mut OpenH264Encoder| {
            for _ in 0..15 {
                encode(encoder, &busy(320, 180, n), n, false);
                n += 1;
            }
            let before = out.lock().unwrap().len();
            for _ in 0..15 {
                encode(encoder, &busy(320, 180, n), n, false);
                n += 1;
            }
            let out = out.lock().unwrap();
            let tail = &out[before..];
            (tail.iter().map(|f| f.data.len()).sum::<usize>(), tail.iter().any(|f| f.keyframe))
        };
        let (high, _) = run(&mut encoder);
        encoder.set_bitrate(200_000);
        let (low, restarted) = run(&mut encoder);
        assert!(low * 3 < high, "half a second at 200 kbps ({low} bytes) should be much smaller than at 2000 kbps ({high} bytes)");
        assert!(!restarted, "changing the bitrate does not start over with a keyframe");
    }

    #[test]
    fn the_bitrate_goes_up_past_where_it_started_and_back() {
        // abre abaixo do teto, como quando o primeiro espectador chega e a
        // estimativa de banda ainda é baixa
        let (mut encoder, out) = open(320, 180, 300_000);
        let mut n = 0;
        // os bytes da segunda metade de 30 frames, como no teste acima
        let mut run = |encoder: &mut OpenH264Encoder| {
            for _ in 0..15 {
                encode(encoder, &busy(320, 180, n), n, false);
                n += 1;
            }
            let before = out.lock().unwrap().len();
            for _ in 0..15 {
                encode(encoder, &busy(320, 180, n), n, false);
                n += 1;
            }
            out.lock().unwrap()[before..].iter().map(|f| f.data.len()).sum::<usize>()
        };
        run(&mut encoder);
        // a rede melhorou, piorou e melhorou de novo, duas vezes. Os três
        // bitrates ficam abaixo do que esta imagem pequena consegue gastar.
        for round in 1..=2 {
            encoder.set_bitrate(600_000);
            let high = run(&mut encoder);
            encoder.set_bitrate(100_000);
            let low = run(&mut encoder);
            encoder.set_bitrate(300_000);
            let middle = run(&mut encoder);
            assert!(low * 2 < middle, "round {round}: 300 kbps ({middle} bytes) against 100 kbps ({low} bytes)");
            assert!(middle * 3 < high * 2, "round {round}: 600 kbps ({high} bytes) against 300 kbps ({middle} bytes)");
        }
        assert_eq!(encoder.shared.refused.load(Ordering::Relaxed), 0, "OpenH264 took every change");
    }

    /// Quantos frames por segundo o codificador aguenta, sem o ritmo da captura:
    /// `cargo test encode_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn encode_speed() {
        for (w, h) in [(1280, 720), (1920, 1080)] {
            for (name, frame) in [("moving bars", pattern as fn(u32, u32, u32) -> VideoPacket), ("bars with noise", busy)] {
                let frames: Vec<VideoPacket> = (0..60).map(|n| frame(w, h, n)).collect();
                let (mut encoder, out) = open(w, h, 6_000_000);
                let started = Instant::now();
                for (n, f) in frames.iter().enumerate() {
                    encode(&mut encoder, f, n as u32, false);
                }
                let took = started.elapsed();
                println!("{}", encoder.take_timings().unwrap_or_default());
                drop(encoder);
                let out = out.lock().unwrap();
                println!(
                    "{w}x{h}, {name}: {:.0} fps ({:.1} ms a frame), {} of {} frames out, {} KB",
                    frames.len() as f64 / took.as_secs_f64(),
                    took.as_secs_f64() * 1000.0 / frames.len() as f64,
                    out.len(),
                    frames.len(),
                    out.iter().map(|f| f.data.len()).sum::<usize>() / 1024
                );
            }
        }
    }

    #[test]
    fn refuses_a_frame_of_another_size() {
        let (mut encoder, _out) = open(320, 180, 500_000);
        let error = encoder.encode(&pattern(640, 360, 0), Duration::ZERO, false).unwrap_err();
        assert!(error.to_string().contains("640x360"), "{error}");
    }
}
