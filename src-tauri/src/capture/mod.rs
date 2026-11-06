//! Captura nativa de telas, janelas e som. Junta as peças de `contracts.rs`:
//! a fonte de vídeo e de áudio do sistema (hoje `win/`), a conversão e o
//! destino dos pacotes (hoje o WebSocket local de `sink.rs`, que o app
//! transforma em faixas para o WebRTC).

pub mod commands;
pub mod contracts;
mod convert;
pub mod packet;
mod sink;
mod win;

use contracts::{AudioSource, FrameSink, VideoConfig, VideoSource};
pub use contracts::Source;
use sink::WebSocketSink;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};

/// Peças específicas do sistema (prioridade de thread etc.).
use win as platform;

static VIDEO: win::WgcSource = win::WgcSource;
static AUDIO: win::WasapiLoopback = win::WasapiLoopback;

pub use sink::serve;

struct Session {
    token: String,
    stop: Arc<AtomicBool>,
    stop_video: Option<Box<dyn FnOnce() + Send>>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn random_token() -> String {
    use std::hash::{BuildHasher, RandomState};
    let a = RandomState::new().hash_one(Instant::now());
    let b = RandomState::new().hash_one(std::process::id());
    format!("{a:016x}{b:016x}")
}

pub fn list_sources() -> Vec<Source> {
    VIDEO.list()
}

pub fn thumbnail(id: &str) -> Option<String> {
    VIDEO.thumbnail(id)
}

pub fn stop() {
    stop_if(None);
}

/// Começa a capturar `id`; devolve a URL do WebSocket local com os frames.
/// Com `native_send` (bitrate máximo), os frames também vão para o envio
/// nativo (`send/`), que codifica e manda direto aos espectadores.
pub fn start(id: &str, height: u32, fps: u32, audio: bool, gpu: bool, native_send: Option<u32>) -> Result<String, String> {
    stop();
    let token = random_token();
    let stop = Arc::new(AtomicBool::new(false));
    let fps = fps.clamp(5, 60);
    let (sink, url) = WebSocketSink::open(&token, fps, stop.clone())?;
    let sink: Arc<dyn FrameSink> = match native_send {
        Some(bitrate) => Arc::new(crate::send::Tee(sink, crate::send::start(fps, bitrate))),
        None => sink,
    };
    let Some(info) = VIDEO.inspect(id) else {
        stop.store(true, Ordering::Relaxed);
        WebSocketSink::discard(&token);
        return Err("fonte inválida".into());
    };
    log_info!(
        "capture",
        "Starting capture of {} at up to {height}p {fps}fps, audio {}, hardware acceleration {}{}",
        info.description,
        if audio { "on" } else { "off" },
        if gpu { "on" } else { "off" },
        if native_send.is_some() { ", native sending (experimental)" } else { "" }
    );

    let config = VideoConfig { max_height: height, fps, gpu };
    let video = match VIDEO.start(id, config, sink.clone(), stop.clone()) {
        Ok(video) => video,
        Err(e) => {
            log_error!("capture", "Windows refused to start the capture: {e}");
            stop.store(true, Ordering::Relaxed);
            WebSocketSink::discard(&token);
            return Err(e);
        }
    };

    if let (true, Some(target)) = (audio, info.audio) {
        let stop = stop.clone();
        thread::spawn(move || {
            if let Err(e) = AUDIO.run(target, sink, stop) {
                log_error!("audio", "Audio capture failed: {e}");
            }
        });
    }

    *SESSION.lock().unwrap() = Some(Session { token, stop, stop_video: Some(video.stop) });
    Ok(url)
}

/// Para os testes de outros módulos: a captura de uma tela mandando os
/// frames direto para `sink` (sem o WebSocket).
#[cfg(test)]
pub fn start_video_for_test(sink: Arc<dyn FrameSink>, height: u32, fps: u32) -> (Arc<AtomicBool>, contracts::RunningVideo) {
    let screen = list_sources().into_iter().find(|s| s.kind == "screen").expect("a screen").id;
    let stop = Arc::new(AtomicBool::new(false));
    let video = VIDEO.start(&screen, VideoConfig { max_height: height, fps, gpu: true }, sink, stop.clone()).unwrap();
    (stop, video)
}

/// Para a sessão atual; com `token`, só se for aquela sessão.
pub fn stop_if(token: Option<&str>) {
    let session = {
        let mut current = SESSION.lock().unwrap();
        match (current.as_ref(), token) {
            (Some(s), Some(t)) if s.token != t => None,
            _ => current.take(),
        }
    };
    if let Some(mut s) = session {
        log_info!("capture", "Capture stopped");
        s.stop.store(true, Ordering::Relaxed);
        WebSocketSink::discard(&s.token);
        if let Some(stop_video) = s.stop_video.take() {
            stop_video();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::packet::{PixelFormat, HEADER};
    use super::*;
    use std::time::Duration;

    /// `cargo test capture_smoke -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn capture_smoke() {
        tokio::spawn(serve());
        tokio::time::sleep(Duration::from_millis(200)).await;

        let sources = list_sources();
        for s in &sources {
            println!("{} [{}] {} — {}", s.id, s.kind, s.title, s.app);
        }
        let pick = std::env::var("CAPTURE_SOURCE").unwrap_or_else(|_| "screen".into());
        let screen = sources
            .iter()
            .find(|s| s.kind == pick || s.title.contains(&pick))
            .unwrap()
            .id
            .clone();
        let height: u32 = std::env::var("CAPTURE_H").ok().and_then(|v| v.parse().ok()).unwrap_or(720);
        let fps: u32 = std::env::var("CAPTURE_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
        let t = Instant::now();
        let thumb = thumbnail(&screen);
        println!("thumbnail: {:?} bytes em {:?}", thumb.map(|t| t.len()), t.elapsed());

        let gpu = std::env::var("CAPTURE_GPU").map_or(true, |v| v != "0");
        let url = start(&screen, height, fps, true, gpu, None).unwrap();
        println!("url: {url}");
        let token = url.rsplit('/').next().unwrap();
        let (mut video, mut audio) = sink::take_waiting(token).unwrap();
        let (mut nv, mut na, mut dims, mut fmt) = (0, 0, (0u16, 0u16), 0u8);
        let mut last = None;
        let secs: u64 = std::env::var("CAPTURE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            tokio::select! {
                Some(v) = video.recv() => { nv += 1; fmt = v[1]; dims = (u16::from_le_bytes([v[2], v[3]]), u16::from_le_bytes([v[4], v[5]])); last = Some(v); }
                Some(_) = audio.recv() => na += 1,
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        stop();
        println!(
            "video frames: {nv} ({}x{}, {}), audio packets: {na}",
            dims.0,
            dims.1,
            if fmt == PixelFormat::Nv12 as u8 { "NV12/GPU" } else { "I420/CPU" }
        );
        if let (Ok(path), Some(pkt)) = (std::env::var("CAPTURE_DUMP"), last) {
            dump_jpeg(&pkt, &path);
            println!("frame salvo em {path}");
        }
        assert!(nv > 0);
    }

    /// Captura a tela e codifica cada frame em H.264 (o caminho do envio
    /// nativo, sem o WebSocket). Mede a captura e o codificador.
    /// `cargo test capture_encode_smoke -- --ignored --nocapture`
    /// (CAPTURE_H, CAPTURE_FPS, CAPTURE_SECS, CAPTURE_GPU=0, ENCODE_DUMP=arquivo.h264)
    #[test]
    #[ignore]
    fn capture_encode_smoke() {
        use crate::encode::{self, EncoderConfig, VideoEncoder};
        use axum::body::Bytes;
        use std::time::Duration;

        #[derive(Default)]
        struct Stats {
            encoder: Option<Box<dyn VideoEncoder>>,
            started: Option<Instant>,
            frames: u32,
            encoded: Vec<u8>,
            keyframes: u32,
            times: Vec<Duration>,
            size: (u16, u16),
        }
        struct EncodingSink {
            fps: u32,
            stats: Mutex<Stats>,
        }
        impl FrameSink for EncodingSink {
            fn video(&self, packet: packet::VideoPacket) {
                let mut s = self.stats.lock().unwrap();
                let started = *s.started.get_or_insert_with(Instant::now);
                s.size = (packet.width(), packet.height());
                if s.encoder.is_none() {
                    let config = EncoderConfig {
                        width: packet.width() as u32,
                        height: packet.height() as u32,
                        fps: self.fps,
                        bitrate: 8_000_000,
                        software_only: false,
                    };
                    s.encoder = Some(encode::open(config).expect("an encoder"));
                }
                let first = s.frames == 0;
                let t = Instant::now();
                let out = s.encoder.as_mut().unwrap().encode(&packet, started.elapsed(), first).unwrap();
                s.times.push(t.elapsed());
                s.frames += 1;
                for f in out {
                    s.keyframes += f.keyframe as u32;
                    s.encoded.extend(f.data);
                }
            }
            fn audio(&self, _: Bytes) {}
            fn is_closed(&self) -> bool {
                false
            }
        }

        let env = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let (height, fps, secs) = (env("CAPTURE_H", 1080), env("CAPTURE_FPS", 60), env("CAPTURE_SECS", 5));
        let gpu = std::env::var("CAPTURE_GPU").map_or(true, |v| v != "0");
        let screen = list_sources().into_iter().find(|s| s.kind == "screen").unwrap().id;
        let sink = Arc::new(EncodingSink { fps, stats: Mutex::default() });
        let stop = Arc::new(AtomicBool::new(false));
        let video = VIDEO
            .start(&screen, VideoConfig { max_height: height, fps, gpu }, sink.clone(), stop.clone())
            .unwrap();
        thread::sleep(Duration::from_secs(secs as u64));
        stop.store(true, Ordering::Relaxed);
        (video.stop)();
        thread::sleep(Duration::from_millis(200));

        let mut s = sink.stats.lock().unwrap();
        let elapsed = s.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(1.0);
        s.times.sort();
        let encoder = s.encoder.as_ref().map(|e| e.describe()).unwrap_or_default();
        println!(
            "{}x{} via {encoder}: {} frames in {elapsed:.1}s ({:.1} fps), {} keyframes, {} KB ({:.0} kbps), encode time median {:?}, 95% {:?}, worst {:?}",
            s.size.0,
            s.size.1,
            s.frames,
            s.frames as f64 / elapsed,
            s.keyframes,
            s.encoded.len() / 1024,
            s.encoded.len() as f64 * 8.0 / elapsed / 1000.0,
            s.times[s.times.len() / 2],
            s.times[s.times.len() * 95 / 100],
            s.times[s.times.len() - 1],
        );
        if let Ok(path) = std::env::var("ENCODE_DUMP") {
            std::fs::write(&path, &s.encoded).unwrap();
            println!("saved to {path}");
        }
        assert!(s.frames > 0 && s.keyframes > 0);
    }

    /// Converte um pacote de vídeo (I420 ou NV12, BT.709 limitado) de volta
    /// para RGB e salva em JPEG, para conferir as cores a olho.
    fn dump_jpeg(pkt: &[u8], path: &str) {
        let w = u16::from_le_bytes([pkt[2], pkt[3]]) as usize;
        let h = u16::from_le_bytes([pkt[4], pkt[5]]) as usize;
        let data = &pkt[HEADER..];
        let (y, uv) = data.split_at(w * h);
        let mut rgb = vec![0u8; w * h * 3];
        for row in 0..h {
            for col in 0..w {
                let ci = (row / 2) * (w / 2) + col / 2;
                let (u, v) = if pkt[1] == PixelFormat::Nv12 as u8 {
                    let i = (row / 2) * w + (col / 2) * 2;
                    (uv[i], uv[i + 1])
                } else {
                    (uv[ci], uv[w * h / 4 + ci])
                };
                let yf = (y[row * w + col] as f32 - 16.0) * 1.164;
                let (uf, vf) = (u as f32 - 128.0, v as f32 - 128.0);
                let px = &mut rgb[(row * w + col) * 3..][..3];
                px[0] = (yf + 1.793 * vf).clamp(0.0, 255.0) as u8;
                px[1] = (yf - 0.213 * uf - 0.533 * vf).clamp(0.0, 255.0) as u8;
                px[2] = (yf + 2.112 * uf).clamp(0.0, 255.0) as u8;
            }
        }
        jpeg_encoder::Encoder::new(std::fs::File::create(path).unwrap(), 85)
            .encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
            .unwrap();
    }
}
