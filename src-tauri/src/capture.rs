//! Captura nativa de telas e janelas (Windows Graphics Capture) e de áudio
//! por processo (WASAPI process loopback). Os frames vão para o frontend
//! por um WebSocket local; lá viram MediaStreamTracks para o WebRTC.

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::Engine;
use futures_util::SinkExt;
use fast_image_resize as fr;
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::VecDeque,
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const PKT_VIDEO: u8 = 1;
const PKT_AUDIO: u8 = 2;
/// Cabeçalho dos pacotes: [tipo, _, largura u16, altura u16, ...] = 16 bytes.
const HEADER: usize = 16;

// ---------------------------------------------------------------------------
// Fontes (telas e janelas)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Source {
    id: String,
    kind: &'static str,
    title: String,
    app: String,
}

enum Target {
    Window(Window),
    Monitor(Monitor),
}

fn parse_target(id: &str) -> Option<Target> {
    let (kind, raw) = id.split_once(':')?;
    let raw = raw.parse::<usize>().ok()? as *mut c_void;
    match kind {
        "w" => Some(Target::Window(Window::from_raw_hwnd(raw))),
        "m" => Some(Target::Monitor(Monitor::from_raw_hmonitor(raw))),
        _ => None,
    }
}

fn is_cloaked(hwnd: *mut c_void) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    let mut cloaked: u32 = 0;
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd as _,
            DWMWA_CLOAKED as _,
            &mut cloaked as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr == 0 && cloaked != 0
}

pub fn list_sources() -> Vec<Source> {
    let mut out = Vec::new();

    if let Ok(monitors) = Monitor::enumerate() {
        for (i, m) in monitors.iter().enumerate() {
            let size = format!("{}×{}", m.width().unwrap_or(0), m.height().unwrap_or(0));
            out.push(Source {
                id: format!("m:{}", m.as_raw_hmonitor() as usize),
                kind: "screen",
                title: (i + 1).to_string(), // o frontend traduz ("Tela 1")
                app: size,
            });
        }
    }

    let me = std::process::id();
    if let Ok(windows) = Window::enumerate() {
        for w in windows {
            let Ok(title) = w.title() else { continue };
            let title = title.trim().to_string();
            if title.is_empty() || title == "Program Manager" {
                continue;
            }
            if w.process_id().ok() == Some(me) || is_cloaked(w.as_raw_hwnd()) {
                continue;
            }
            let app = w.process_name().unwrap_or_default();
            out.push(Source {
                id: format!("w:{}", w.as_raw_hwnd() as usize),
                kind: "window",
                title,
                app: app
                    .strip_suffix(".exe")
                    .or_else(|| app.strip_suffix(".EXE"))
                    .unwrap_or(&app)
                    .to_string(),
            });
        }
    }
    out
}

/// Inicia uma sessão WGC (janela ou monitor) com o handler `H`.
fn start_session<H>(
    target: Target,
    fps: Option<u32>,
    flags: H::Flags,
) -> Result<CaptureControl<H, BoxError>, String>
where
    H: GraphicsCaptureApiHandler<Error = BoxError> + Send + 'static,
    H::Flags: Send,
{
    let border = if GraphicsCaptureApi::is_border_settings_supported().unwrap_or(false) {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    };
    let interval = match fps {
        Some(fps) if GraphicsCaptureApi::is_minimum_update_interval_supported().unwrap_or(false) => {
            MinimumUpdateIntervalSettings::Custom(Duration::from_micros(1_000_000 / fps as u64))
        }
        _ => MinimumUpdateIntervalSettings::Default,
    };

    macro_rules! go {
        ($item:expr) => {
            H::start_free_threaded(Settings::new(
                $item,
                CursorCaptureSettings::WithCursor,
                border,
                SecondaryWindowSettings::Default,
                interval,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                flags,
            ))
            .map_err(|e| format!("falha ao iniciar a captura: {e}"))
        };
    }
    match target {
        Target::Window(w) => go!(w),
        Target::Monitor(m) => go!(m),
    }
}

/// Copia o frame (sem padding) e redimensiona para caber em `max_w` x `max_h`,
/// mantendo a proporção. Retorna (largura, altura, bgra).
fn scaled_bgra(
    frame: &mut Frame,
    max_w: u32,
    max_h: u32,
    resizer: &mut fr::Resizer,
    scratch: &mut Vec<u8>,
) -> Result<(u32, u32, Vec<u8>), BoxError> {
    let mut buf = frame.buffer()?;
    let (sw, sh) = (buf.width(), buf.height());
    let src: &[u8] = if buf.has_padding() {
        buf.as_nopadding_buffer(scratch)
    } else {
        buf.as_raw_buffer()
    };

    let scale = (max_w as f64 / sw as f64).min(max_h as f64 / sh as f64).min(1.0);
    // dimensões pares (exigência do I420)
    let dw = (((sw as f64 * scale) as u32) & !1).max(2);
    let dh = (((sh as f64 * scale) as u32) & !1).max(2);

    if (dw, dh) == (sw, sh) {
        return Ok((dw, dh, src[..(dw * dh * 4) as usize].to_vec()));
    }
    let src_img = fr::images::ImageRef::new(sw, sh, src, fr::PixelType::U8x4)?;
    let mut dst = fr::images::Image::new(dw, dh, fr::PixelType::U8x4);
    let opts = fr::ResizeOptions::new()
        .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear))
        .use_alpha(false);
    resizer.resize(&src_img, &mut dst, &opts)?;
    Ok((dw, dh, dst.into_vec()))
}

/// BGRA -> I420 (BT.709, faixa limitada), em paralelo por pares de linhas.
fn bgra_to_i420(w: usize, h: usize, bgra: &[u8], out: &mut [u8]) {
    let (y_plane, uv) = out.split_at_mut(w * h);
    let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
    let cw = w / 2;

    y_plane
        .par_chunks_mut(w * 2)
        .zip(u_plane.par_chunks_mut(cw))
        .zip(v_plane.par_chunks_mut(cw))
        .enumerate()
        .for_each(|(row, ((y2, u), v))| {
            let src0 = &bgra[row * 2 * w * 4..][..w * 4];
            let src1 = &bgra[(row * 2 + 1) * w * 4..][..w * 4];
            let (y0, y1) = y2.split_at_mut(w);
            for x in 0..cw {
                let mut rs = 0i32;
                let mut gs = 0i32;
                let mut bs = 0i32;
                for (src, ys) in [(src0, &mut *y0), (src1, &mut *y1)] {
                    for dx in 0..2 {
                        let px = x * 2 + dx;
                        let b = src[px * 4] as i32;
                        let g = src[px * 4 + 1] as i32;
                        let r = src[px * 4 + 2] as i32;
                        ys[px] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
                        rs += r;
                        gs += g;
                        bs += b;
                    }
                }
                let (r, g, b) = (rs / 4, gs / 4, bs / 4);
                u[x] = (((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
                v[x] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            }
        });
}

// ---------------------------------------------------------------------------
// Miniaturas
// ---------------------------------------------------------------------------

struct Thumb {
    tx: Option<std::sync::mpsc::Sender<String>>,
    resizer: fr::Resizer,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Thumb {
    type Flags = std::sync::mpsc::Sender<String>;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { tx: Some(ctx.flags), resizer: fr::Resizer::new(), scratch: Vec::new() })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.tx.take() {
            let (w, h, bgra) = scaled_bgra(frame, 400, 225, &mut self.resizer, &mut self.scratch)?;
            let mut jpeg = Vec::new();
            jpeg_encoder::Encoder::new(&mut jpeg, 75).encode(
                &bgra,
                w as u16,
                h as u16,
                jpeg_encoder::ColorType::Bgra,
            )?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
            let _ = tx.send(format!("data:image/jpeg;base64,{b64}"));
        }
        control.stop();
        Ok(())
    }
}

pub fn thumbnail(id: &str) -> Option<String> {
    let target = parse_target(id)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let control = start_session::<Thumb>(target, None, tx).ok()?;
    let result = rx.recv_timeout(Duration::from_millis(1500)).ok();
    let _ = control.stop();
    result
}

// ---------------------------------------------------------------------------
// Transmissão
// ---------------------------------------------------------------------------

struct StreamFlags {
    max_h: u32,
    latest: Arc<Mutex<Option<Bytes>>>,
    fresh: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

struct Streamer {
    flags: StreamFlags,
    resizer: fr::Resizer,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Streamer {
    type Flags = StreamFlags;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { flags: ctx.flags, resizer: fr::Resizer::new(), scratch: Vec::new() })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.flags.stop.load(Ordering::Relaxed) {
            control.stop();
            return Ok(());
        }
        let max_h = self.flags.max_h;
        let max_w = max_h * 21 / 9; // limita ultrawide
        let (w, h, bgra) = scaled_bgra(frame, max_w, max_h, &mut self.resizer, &mut self.scratch)?;

        let mut pkt = vec![0u8; HEADER + (w * h * 3 / 2) as usize];
        pkt[0] = PKT_VIDEO;
        pkt[2..4].copy_from_slice(&(w as u16).to_le_bytes());
        pkt[4..6].copy_from_slice(&(h as u16).to_le_bytes());
        bgra_to_i420(w as usize, h as usize, &bgra, &mut pkt[HEADER..]);

        *self.flags.latest.lock().unwrap() = Some(Bytes::from(pkt));
        self.flags.fresh.store(true, Ordering::Release);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        // janela fechada: encerra tudo (o frontend percebe pelo WebSocket)
        self.flags.stop.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// Envia frames no ritmo do FPS escolhido; se a imagem ficar parada, reenvia
/// o último frame a cada 500ms para quem entrar depois receber imagem.
fn pacer(
    fps: u32,
    latest: Arc<Mutex<Option<Bytes>>>,
    fresh: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<Bytes>,
) {
    let interval = Duration::from_micros(1_000_000 / fps as u64);
    let mut next = Instant::now();
    let mut last_sent = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        next += interval;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        } else {
            next = Instant::now();
        }
        let is_fresh = fresh.swap(false, Ordering::Acquire);
        if !is_fresh && last_sent.elapsed() < Duration::from_millis(500) {
            continue;
        }
        let Some(frame) = latest.lock().unwrap().clone() else { continue };
        if tx.is_closed() {
            break;
        }
        let _ = tx.try_send(frame); // descarta se o frontend estiver atrasado
        last_sent = Instant::now();
    }
}

/// Áudio do processo alvo (janela) ou de tudo menos este app (tela inteira).
fn audio_loop(
    pid: u32,
    include_tree: bool,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<Bytes>,
) -> Result<(), BoxError> {
    use wasapi::*;
    let _ = initialize_mta();

    let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };

    let client = match AudioClient::new_application_loopback_client(pid, include_tree)
        .and_then(|mut c| c.initialize_client(&format, &Direction::Capture, &mode).map(|_| c))
    {
        Ok(c) => c,
        Err(e) => {
            // Windows antigo sem process loopback: cai para o áudio do sistema todo
            eprintln!("process loopback indisponível ({e}); usando loopback do sistema");
            let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
            let mut c = device.get_iaudioclient()?;
            c.initialize_client(&format, &Direction::Capture, &mode)?;
            c
        }
    };

    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    client.start_stream()?;

    const CHUNK: usize = 480 * 8; // 10ms de f32 estéreo a 48kHz
    let mut queue: VecDeque<u8> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) && !tx.is_closed() {
        let _ = event.wait_for_event(100);
        while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
            let before = queue.len();
            let info = capture.read_from_device_to_deque(&mut queue)?;
            if info.flags.silent {
                queue.range_mut(before..).for_each(|b| *b = 0);
            }
        }
        while queue.len() >= CHUNK {
            let mut pkt = vec![0u8; HEADER];
            pkt[0] = PKT_AUDIO;
            pkt.extend(queue.drain(..CHUNK));
            let _ = tx.try_send(Bytes::from(pkt));
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

struct Session {
    token: String,
    stop: Arc<AtomicBool>,
    channels: Option<(mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>)>,
    control: Option<CaptureControl<Streamer, BoxError>>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static PORT: OnceLock<u16> = OnceLock::new();

fn random_token() -> String {
    use std::hash::{BuildHasher, RandomState};
    let a = RandomState::new().hash_one(Instant::now());
    let b = RandomState::new().hash_one(std::process::id());
    format!("{a:016x}{b:016x}")
}

pub fn stop() {
    stop_if(None);
}

pub fn start(id: &str, height: u32, fps: u32, audio: bool) -> Result<String, String> {
    stop();
    let port = *PORT.get().ok_or("servidor de captura não iniciado")?;
    let target = parse_target(id).ok_or("fonte inválida")?;
    let fps = fps.clamp(5, 60);

    let audio_target = match &target {
        Target::Window(w) => w.process_id().ok().map(|pid| (pid, true)),
        // tela inteira: tudo, menos o som deste app (as streams dos amigos)
        Target::Monitor(_) => Some((std::process::id(), false)),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let latest = Arc::new(Mutex::new(None));
    let fresh = Arc::new(AtomicBool::new(false));
    let (video_tx, video_rx) = mpsc::channel::<Bytes>(2);
    let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(200);

    let control = start_session::<Streamer>(
        target,
        Some(fps),
        StreamFlags {
            max_h: height,
            latest: latest.clone(),
            fresh: fresh.clone(),
            stop: stop.clone(),
        },
    )?;

    {
        let stop = stop.clone();
        thread::spawn(move || pacer(fps, latest, fresh, stop, video_tx));
    }
    match audio_target {
        Some((pid, include_tree)) if audio => {
            let stop = stop.clone();
            thread::spawn(move || {
                if let Err(e) = audio_loop(pid, include_tree, stop, audio_tx) {
                    eprintln!("captura de áudio falhou: {e}");
                }
            });
        }
        _ => drop(audio_tx),
    }

    let token = random_token();
    *SESSION.lock().unwrap() = Some(Session {
        token: token.clone(),
        stop,
        channels: Some((video_rx, audio_rx)),
        control: Some(control),
    });
    Ok(format!("ws://127.0.0.1:{port}/frames/{token}"))
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
        s.stop.store(true, Ordering::Relaxed);
        if let Some(control) = s.control.take() {
            let _ = control.stop();
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket local que entrega os frames ao frontend
// ---------------------------------------------------------------------------

pub async fn serve() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let _ = PORT.set(listener.local_addr()?.port());
    let app = Router::new().route("/frames/{token}", get(frames_handler));
    axum::serve(listener, app).await
}

async fn frames_handler(ws: WebSocketUpgrade, Path(token): Path<String>) -> impl IntoResponse {
    let channels = {
        let mut session = SESSION.lock().unwrap();
        match session.as_mut() {
            Some(s) if s.token == token => s.channels.take(),
            _ => None,
        }
    };
    ws.on_upgrade(move |socket| async move {
        if let Some((video, audio)) = channels {
            pump(socket, video, audio).await;
        }
    })
}

async fn pump(mut socket: WebSocket, mut video: mpsc::Receiver<Bytes>, mut audio: mpsc::Receiver<Bytes>) {
    let mut video_open = true;
    let mut audio_open = true;
    while video_open {
        let msg = tokio::select! {
            m = video.recv() => match m { Some(m) => m, None => { video_open = false; continue } },
            m = audio.recv(), if audio_open => match m { Some(m) => m, None => { audio_open = false; continue } },
        };
        if socket.send(Message::Binary(msg)).await.is_err() {
            break;
        }
    }
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let url = start(&screen, height, fps, true).unwrap();
        println!("url: {url}");
        let (mut video, mut audio) = SESSION.lock().unwrap().as_mut().unwrap().channels.take().unwrap();
        let (mut nv, mut na, mut dims) = (0, 0, (0u16, 0u16));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                Some(v) = video.recv() => { nv += 1; dims = (u16::from_le_bytes([v[2], v[3]]), u16::from_le_bytes([v[4], v[5]])); }
                Some(_) = audio.recv() => na += 1,
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        stop();
        println!("video frames: {nv} ({}x{}), audio packets: {na}", dims.0, dims.1);
        assert!(nv > 0);
    }
}
