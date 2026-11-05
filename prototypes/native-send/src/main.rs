//! Protótipo do envio nativo (ROADMAP, seção 2): o Rust codifica H.264 UMA vez
//! e manda por WebRTC (str0m) para todos os navegadores que abrirem a página.
//! Quem assiste usa o WebRTC normal do navegador, igual ao app.
//!
//! - Vídeo de teste gerado aqui (barras que se mexem), 1280x720 a 30 fps.
//! - Codificação no processador com openh264 (no app seria Media Foundation,
//!   pela placa de vídeo). Perfil Constrained Baseline 42e01f.
//! - Quem transmite faz a oferta (como no app); a página responde.
//! - Controle de banda: a estimativa (TWCC) de cada espectador vai para o
//!   codificador, que usa a menor delas (uma codificação só para todos).
//!
//! Rodar: `cargo run --release` e abrir http://<ip>:3010 em uma ou mais abas.

use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use openh264::{
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, RateControlMode, UsageType},
    formats::YUVBuffer,
    OpenH264API,
};
use str0m::{
    bwe::{Bitrate, BweKind},
    change::{SdpAnswer, SdpPendingOffer},
    format::Codec,
    media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt},
    net::{Protocol, Receive},
    Candidate, Event, IceConnectionState, Input, Output, Rtc,
};

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const FPS: u32 = 30;
const START_BITRATE: u64 = 1_500_000;
const MIN_BITRATE: u64 = 100_000;
/// No máximo um quadro-chave por esse intervalo (o libwebrtc faz o mesmo):
/// sem isso, um espectador com perda pede dezenas por segundo e piora tudo.
const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_millis(1000);
const MAX_BITRATE: u64 = 6_000_000;

/// Um frame já codificado, compartilhado por todos os espectadores.
#[derive(Clone)]
struct EncodedFrame {
    data: Arc<[u8]>,
    rtp_time: MediaTime,
    captured: Instant,
}

/// Pedidos dos espectadores ao codificador.
enum EncoderCmd {
    Keyframe(u32),
}

#[derive(Default)]
struct Shared {
    next_id: u32,
    /// Conexões criadas que ainda esperam a resposta da página.
    pending: HashMap<u32, (Rtc, SdpPendingOffer, UdpSocket, Mid)>,
    /// Para onde mandar os frames de cada espectador.
    viewers: HashMap<u32, Sender<EncodedFrame>>,
    /// Banda estimada para cada espectador (bps).
    estimates: HashMap<u32, u64>,
}

type State = Arc<Mutex<Shared>>;

fn log(msg: impl AsRef<str>) {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        % 100_000_000;
    println!("{:>5}.{:03}  {}", (t / 1000) % 100_000, t % 1000, msg.as_ref());
}

fn main() {
    str0m::crypto::from_feature_flags().install_process_default();

    let state: State = Arc::default();
    let (cmd_tx, cmd_rx) = mpsc::channel();
    {
        let state = state.clone();
        thread::spawn(move || encode_loop(state, cmd_rx));
    }

    let host = lan_ip();
    let server = tiny_http::Server::http("0.0.0.0:3010").expect("port 3010 is free");
    log(format!("Open http://{host}:3010 in one or more browser tabs"));
    for mut req in server.incoming_requests() {
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let (status, ctype, out) = match (req.method().as_str(), req.url()) {
            ("GET", "/") => (200, "text/html; charset=utf-8", include_str!("viewer.html").to_string()),
            ("POST", "/offer") => match make_offer(&state, host) {
                Ok(json) => (200, "application/json", json),
                Err(e) => (500, "text/plain", e),
            },
            ("POST", url) if url.starts_with("/answer/") => {
                let id = url["/answer/".len()..].parse().unwrap_or(0);
                match accept_answer(&state, id, &body, cmd_tx.clone()) {
                    Ok(()) => (200, "text/plain", "ok".into()),
                    Err(e) => (400, "text/plain", e),
                }
            }
            _ => (404, "text/plain", "not found".into()),
        };
        let header = tiny_http::Header::from_bytes("Content-Type", ctype).unwrap();
        let _ = req.respond(tiny_http::Response::from_string(out).with_status_code(status).with_header(header));
    }
}

/// Endereço da placa de rede usada para sair para a internet.
fn lan_ip() -> IpAddr {
    let s = UdpSocket::bind("0.0.0.0:0").unwrap();
    s.connect("8.8.8.8:80").unwrap();
    s.local_addr().unwrap().ip()
}

/// Cria a conexão de um novo espectador e devolve a oferta (quem transmite oferece).
fn make_offer(state: &State, host: IpAddr) -> Result<String, String> {
    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_h264(true)
        .enable_bwe(Some(Bitrate::bps(START_BITRATE)))
        .set_stats_interval(Some(Duration::from_secs(5)))
        .build(Instant::now());

    let socket = UdpSocket::bind(SocketAddr::new(host, 0)).map_err(|e| e.to_string())?;
    let candidate = Candidate::host(socket.local_addr().unwrap(), "udp").map_err(|e| e.to_string())?;
    rtc.add_local_candidate(candidate);

    let mut api = rtc.sdp_api();
    let mid = api.add_media(MediaKind::Video, Direction::SendOnly, Some("screen".into()), Some("video".into()), None);
    let (offer, pending) = api.apply().ok_or("nothing to offer")?;

    let mut s = state.lock().unwrap();
    s.next_id += 1;
    let id = s.next_id;
    s.pending.insert(id, (rtc, pending, socket, mid));
    log(format!("Viewer #{id}: sent the offer"));
    Ok(serde_json::json!({ "id": id, "offer": offer }).to_string())
}

fn accept_answer(state: &State, id: u32, body: &str, cmd: Sender<EncoderCmd>) -> Result<(), String> {
    let (mut rtc, pending, socket, mid) = state.lock().unwrap().pending.remove(&id).ok_or("unknown viewer")?;
    let answer: SdpAnswer = serde_json::from_str(body).map_err(|e| e.to_string())?;
    rtc.sdp_api().accept_answer(pending, answer).map_err(|e| e.to_string())?;
    // o mid só vale depois da resposta; MediaAdded só vem para mídia que o outro lado cria
    let pt = h264_pt(&mut rtc, mid).ok_or("the viewer did not accept H.264")?;
    log(format!("Viewer #{id}: got the answer, video section {mid:?}, H.264 payload type {pt:?}"));

    let (tx, rx) = mpsc::channel();
    state.lock().unwrap().viewers.insert(id, tx);
    let state = state.clone();
    thread::spawn(move || {
        if let Err(e) = viewer_loop(id, rtc, socket, (mid, pt), rx, cmd, &state) {
            log(format!("Viewer #{id}: stopped with an error: {e}"));
        }
        let mut s = state.lock().unwrap();
        s.viewers.remove(&id);
        s.estimates.remove(&id);
        log(format!("Viewer #{id}: gone ({} left)", s.viewers.len()));
    });
    Ok(())
}

/// O frame (Annex B, com 00 00 01 antes de cada NAL) tem um quadro IDR (tipo 5)?
fn has_idr(annex_b: &[u8]) -> bool {
    annex_b.windows(4).any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] & 0x1f == 5)
}

/// Escolhe o H.264 Constrained Baseline com packetization-mode 1.
fn h264_pt(rtc: &mut Rtc, mid: Mid) -> Option<Pt> {
    let writer = rtc.writer(mid)?;
    let params: Vec<_> = writer.payload_params().cloned().collect();
    let h264 = |p: &&str0m::format::PayloadParams| p.spec().codec == Codec::H264;
    params
        .iter()
        .filter(h264)
        .find(|p| {
            let f = p.spec().format;
            f.packetization_mode == Some(1) && f.profile_level_id.map(|id| id >> 8) == Some(0x42e0)
        })
        .or_else(|| params.iter().find(h264))
        .map(|p| p.pt())
}

fn viewer_loop(
    id: u32,
    mut rtc: Rtc,
    socket: UdpSocket,
    video: (Mid, Pt),
    frames: Receiver<EncodedFrame>,
    cmd: Sender<EncoderCmd>,
    state: &State,
) -> Result<(), Box<dyn std::error::Error>> {
    let local = socket.local_addr()?;
    let started = Instant::now();
    let mut buf = vec![0u8; 2000];
    let mut connected = false;
    let mut sent_frames = 0u64;
    let mut waiting_keyframe = true;
    // teste: LIMIT_KBPS simula uma rede lenta (descarta o que passar do limite)
    // só para os espectadores de número par
    let limit_bps: Option<f64> = std::env::var("LIMIT_KBPS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|_| id.is_multiple_of(2))
        .map(|k| k * 1000.0);
    if let Some(l) = limit_bps {
        log(format!("Viewer #{id}: simulating a network limited to {} kbps", l / 1000.0));
    }
    let mut bucket = 0.0f64; // bits disponíveis
    let mut bucket_at = Instant::now();
    let mut dropped = 0u64;
    let mut last_estimate = 0u64;

    loop {
        // manda os frames que chegaram do codificador
        loop {
            match frames.try_recv() {
                Ok(frame) => {
                    let (mid, pt) = video;
                    if !connected {
                        continue;
                    }
                    if waiting_keyframe {
                        // só começa num quadro-chave (senão o navegador não decodifica)
                        if !has_idr(&frame.data) {
                            continue;
                        }
                        waiting_keyframe = false;
                        log(format!("Viewer #{id}: first keyframe out after {:?}", started.elapsed()));
                    }
                    if let Some(w) = rtc.writer(mid) {
                        w.write(pt, frame.captured, frame.rtp_time, frame.data.clone())?;
                        sent_frames += 1;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        let timeout = match rtc.poll_output()? {
            Output::Timeout(t) => t,
            Output::Transmit(t) => {
                if let Some(limit) = limit_bps {
                    let now = Instant::now();
                    // fila de ~100 ms, como um roteador
                    bucket = (bucket + now.duration_since(bucket_at).as_secs_f64() * limit).min(limit * 0.1);
                    bucket_at = now;
                    let bits = (t.contents.len() * 8) as f64;
                    if bucket < bits {
                        dropped += 1;
                        continue;
                    }
                    bucket -= bits;
                }
                socket.send_to(&t.contents, t.destination)?;
                continue;
            }
            Output::Event(e) => {
                match e {
                    Event::IceConnectionStateChange(s) => {
                        log(format!("Viewer #{id}: ICE {s:?} after {:?}", started.elapsed()));
                        if s == IceConnectionState::Disconnected {
                            return Ok(());
                        }
                    }
                    Event::Connected => {
                        connected = true;
                        log(format!("Viewer #{id}: connected (ICE + DTLS) after {:?}", started.elapsed()));
                        let _ = cmd.send(EncoderCmd::Keyframe(id));
                    }
                    Event::KeyframeRequest(r) => {
                        log(format!("Viewer #{id}: asked for a keyframe ({:?})", r.kind));
                        let _ = cmd.send(EncoderCmd::Keyframe(id));
                    }
                    Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. }) => {
                        let est = estimate.as_u64();
                        if est.abs_diff(last_estimate) * 5 > last_estimate {
                            log(format!("Viewer #{id}: bandwidth estimate {} kbps", est / 1000));
                            last_estimate = est;
                        }
                        state.lock().unwrap().estimates.insert(id, est);
                    }
                    Event::MediaEgressStats(s) => log(format!(
                        "Viewer #{id}: {sent_frames} frames sent, {} KB, {} packets, {} resend requests, \
                         {} keyframe requests, round trip {:?}, loss {:.1}%, {dropped} packets dropped by the test limit",
                        s.bytes / 1024,
                        s.packets,
                        s.nacks,
                        s.plis + s.firs,
                        s.rtt,
                        s.loss.unwrap_or(0.0) * 100.0,
                    )),
                    _ => {}
                }
                continue;
            }
        };

        // espera rede no máximo 5 ms para não atrasar os frames
        let wait = timeout.saturating_duration_since(Instant::now()).min(Duration::from_millis(5));
        if wait.is_zero() {
            rtc.handle_input(Input::Timeout(Instant::now()))?;
            continue;
        }
        socket.set_read_timeout(Some(wait))?;
        let input = match socket.recv_from(&mut buf) {
            Ok((n, source)) => Input::Receive(
                Instant::now(),
                Receive { proto: Protocol::Udp, source, destination: local, contents: buf[..n].try_into()? },
            ),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Input::Timeout(Instant::now()),
            Err(e) => return Err(e.into()),
        };
        rtc.handle_input(input)?;
    }
}

/// Gera o vídeo de teste e codifica UMA vez para todos.
fn encode_loop(state: State, cmds: Receiver<EncoderCmd>) {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(START_BITRATE as u32))
        .max_frame_rate(FrameRate::from_hz(FPS as f32))
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .skip_frames(true);
    let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).expect("openh264 encoder");
    let frame_time = Duration::from_secs(1) / FPS;
    let start = Instant::now();
    let mut bitrate = START_BITRATE;
    let mut last_rate_check = Instant::now();
    let mut encode_time = Duration::ZERO;
    let mut n: u64 = 0;
    let mut last_keyframe = Instant::now() - KEYFRAME_MIN_INTERVAL;
    let mut keyframe_waiting: Vec<u32> = Vec::new();

    loop {
        let due = start + frame_time * n as u32;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        n += 1;

        while let Ok(EncoderCmd::Keyframe(id)) = cmds.try_recv() {
            if !keyframe_waiting.contains(&id) {
                keyframe_waiting.push(id);
            }
        }
        let mut keyframe_for = Vec::new();
        if !keyframe_waiting.is_empty() && last_keyframe.elapsed() >= KEYFRAME_MIN_INTERVAL {
            keyframe_for = std::mem::take(&mut keyframe_waiting);
            last_keyframe = Instant::now();
            encoder.force_intra_frame();
        }

        // banda: a menor estimativa entre os espectadores, com folga de 10%
        if last_rate_check.elapsed() >= Duration::from_secs(1) {
            last_rate_check = Instant::now();
            let lowest = state.lock().unwrap().estimates.values().copied().min();
            if let Some(est) = lowest {
                let target = (est * 9 / 10).clamp(MIN_BITRATE, MAX_BITRATE);
                if target.abs_diff(bitrate) * 10 > bitrate {
                    log(format!(
                        "Encoder: bitrate {} -> {} kbps (lowest viewer estimate {} kbps); \
                         average encode time {:.1} ms",
                        bitrate / 1000,
                        target / 1000,
                        est / 1000,
                        encode_time.as_secs_f64() * 1000.0 / n as f64,
                    ));
                    bitrate = target;
                    set_bitrate(&mut encoder, bitrate);
                }
            }
        }

        let yuv = test_pattern(n);
        let t0 = Instant::now();
        let data = match encoder.encode(&yuv) {
            Ok(bits) => bits.to_vec(),
            Err(e) => {
                log(format!("Encoder: failed: {e}"));
                continue;
            }
        };
        encode_time += t0.elapsed();
        if data.is_empty() {
            continue; // quadro pulado pelo controle de banda
        }
        if !keyframe_for.is_empty() {
            log(format!("Encoder: keyframe for viewers {keyframe_for:?} ({} KB)", data.len() / 1024));
        }

        let frame = EncodedFrame {
            data: data.into(),
            rtp_time: MediaTime::new(n * 90_000 / FPS as u64, Frequency::NINETY_KHZ),
            captured: Instant::now(),
        };
        for tx in state.lock().unwrap().viewers.values() {
            let _ = tx.send(frame.clone());
        }
    }
}

fn set_bitrate(encoder: &mut Encoder, bps: u64) {
    use openh264_sys2::{SBitrateInfo, ENCODER_OPTION_BITRATE, SPATIAL_LAYER_ALL};
    let mut info = SBitrateInfo { iLayer: SPATIAL_LAYER_ALL, iBitrate: bps as i32 };
    unsafe {
        encoder.raw_api().set_option(ENCODER_OPTION_BITRATE, (&raw mut info).cast());
    }
}

/// Barras coloridas que andam, um quadrado branco e linhas finas (parecido com
/// tela: bordas nítidas, que é o difícil para o codificador).
fn test_pattern(n: u64) -> YUVBuffer {
    let (w, h) = (WIDTH, HEIGHT);
    let mut yuv = vec![0u8; w * h * 3 / 2];
    let shift = (n * 4) as usize;
    let (y_plane, uv) = yuv.split_at_mut(w * h);
    for y in 0..h {
        for x in 0..w {
            let bar = ((x + shift) / 160) % 8;
            let mut v = 40 + bar as u8 * 25;
            if y % 40 == 0 || x % 64 == 0 {
                v = 230; // linhas finas
            }
            y_plane[y * w + x] = v;
        }
    }
    let bx = (n as usize * 7) % (w - 120);
    let by = (n as usize * 3) % (h - 120);
    for y in by..by + 120 {
        y_plane[y * w + bx..y * w + bx + 120].fill(255);
    }
    let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let bar = ((x * 2 + shift) / 160) % 8;
            u_plane[y * w / 2 + x] = (64 + bar * 20) as u8;
            v_plane[y * w / 2 + x] = (200 - bar * 18) as u8;
        }
    }
    YUVBuffer::from_vec(yuv, w, h)
}
