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
mod gpu_convert;
use gpu_convert::GpuConverter;
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
/// Cabeçalho dos pacotes: [tipo, formato, largura u16, altura u16, ...] = 16 bytes.
const HEADER: usize = 16;
const FMT_I420: u8 = 0;
const FMT_NV12: u8 = 1;

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
    let border_supported = GraphicsCaptureApi::is_border_settings_supported().unwrap_or(false);
    let interval_supported = GraphicsCaptureApi::is_minimum_update_interval_supported().unwrap_or(false);
    if fps.is_some() {
        log_info!(
            "capture",
            "Windows capture features: hide yellow border = {}, frame rate limit = {}",
            if border_supported { "yes" } else { "no (older Windows)" },
            if interval_supported { "yes" } else { "no (older Windows)" },
        );
    }
    let border = if border_supported {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    };
    let interval = match fps {
        Some(fps) if interval_supported => {
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

/// Tamanho final que cabe em `max_w` x `max_h` mantendo a proporção, sem
/// ampliar e com dimensões pares (exigência do I420/NV12).
fn target_size(sw: u32, sh: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f64 / sw as f64).min(max_h as f64 / sh as f64).min(1.0);
    let dw = (((sw as f64 * scale) as u32) & !1).max(2);
    let dh = (((sh as f64 * scale) as u32) & !1).max(2);
    (dw, dh)
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

    let (dw, dh) = target_size(sw, sh, max_w, max_h);

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

/// Baixa a prioridade da thread atual, para a captura não disputar o
/// processador com o jogo.
fn lower_thread_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// Poucas threads e de baixa prioridade para a conversão no processador: o
/// pool global do rayon usaria todos os núcleos a cada frame.
fn convert_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 4).clamp(2, 4))
            .thread_name(|i| format!("capture-convert-{i}"))
            .start_handler(|_| lower_thread_priority())
            .build()
            .expect("pool de conversão")
    })
}

/// BGRA -> I420 (BT.709, faixa limitada), em paralelo por pares de linhas.
fn bgra_to_i420(w: usize, h: usize, bgra: &[u8], out: &mut [u8]) {
    convert_pool().install(|| bgra_to_i420_rows(w, h, bgra, out));
}

fn bgra_to_i420_rows(w: usize, h: usize, bgra: &[u8], out: &mut [u8]) {
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
    /// Converter na placa de vídeo (aceleração de hardware ligada).
    gpu: bool,
    latest: Arc<Mutex<Option<Bytes>>>,
    fresh: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

struct Streamer {
    flags: StreamFlags,
    resizer: fr::Resizer,
    scratch: Vec<u8>,
    frames: u64,
    gpu: Option<GpuConverter>,
    /// A conversão na placa falhou uma vez: segue no processador.
    gpu_failed: bool,
    /// Tempo gasto processando frames desde o último relatório no log.
    busy: Duration,
    busy_frames: u32,
    next_report: Instant,
}

impl GraphicsCaptureApiHandler for Streamer {
    type Flags = StreamFlags;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            gpu_failed: !ctx.flags.gpu,
            flags: ctx.flags,
            resizer: fr::Resizer::new(),
            scratch: Vec::new(),
            frames: 0,
            gpu: None,
            busy: Duration::ZERO,
            busy_frames: 0,
            next_report: Instant::now() + Duration::from_secs(10),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        self.frames += 1;
        if self.flags.stop.load(Ordering::Relaxed) {
            control.stop();
            return Ok(());
        }
        if self.frames == 1 {
            lower_thread_priority();
        }
        let max_h = self.flags.max_h;
        let max_w = max_h * 21 / 9; // limita ultrawide
        let src = (frame.width(), frame.height());
        let started = Instant::now();
        let pkt = match self.convert_on_gpu(frame, max_w, max_h) {
            Some(pkt) => pkt,
            None => {
                let (w, h, bgra) = scaled_bgra(frame, max_w, max_h, &mut self.resizer, &mut self.scratch)
                    .inspect_err(|e| log_error!("capture", "Could not process a captured frame: {e}"))?;
                let mut pkt = video_packet(FMT_I420, w, h);
                bgra_to_i420(w as usize, h as usize, &bgra, &mut pkt[HEADER..]);
                pkt
            }
        };
        self.report_time(started.elapsed(), pkt[1]);
        if self.frames == 1 {
            let w = u16::from_le_bytes([pkt[2], pkt[3]]);
            let h = u16::from_le_bytes([pkt[4], pkt[5]]);
            log_info!(
                "capture",
                "First frame captured: {}x{}, sent as {w}x{h}, converted on the {}",
                src.0,
                src.1,
                if pkt[1] == FMT_NV12 { "GPU" } else { "CPU" }
            );
        }

        *self.flags.latest.lock().unwrap() = Some(Bytes::from(pkt));
        self.flags.fresh.store(true, Ordering::Release);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        // janela fechada: encerra tudo (o frontend percebe pelo WebSocket)
        log_info!("capture", "The captured window or screen went away, stopping ({} frames captured)", self.frames);
        self.flags.stop.store(true, Ordering::Relaxed);
        Ok(())
    }
}

fn video_packet(format: u8, w: u32, h: u32) -> Vec<u8> {
    let mut pkt = vec![0u8; HEADER + (w * h * 3 / 2) as usize];
    pkt[0] = PKT_VIDEO;
    pkt[1] = format;
    pkt[2..4].copy_from_slice(&(w as u16).to_le_bytes());
    pkt[4..6].copy_from_slice(&(h as u16).to_le_bytes());
    pkt
}

impl Streamer {
    /// De tempos em tempos, diz no log quanto custa processar cada frame.
    fn report_time(&mut self, took: Duration, format: u8) {
        self.busy += took;
        self.busy_frames += 1;
        if Instant::now() < self.next_report {
            return;
        }
        log_info!(
            "capture",
            "Processing a frame takes {:.1} ms on average on the {} ({} frames)",
            self.busy.as_secs_f64() * 1000.0 / self.busy_frames as f64,
            if format == FMT_NV12 { "GPU" } else { "CPU" },
            self.busy_frames
        );
        self.busy = Duration::ZERO;
        self.busy_frames = 0;
        self.next_report = Instant::now() + Duration::from_secs(300);
    }

    /// Converte na placa de vídeo; `None` = use o processador (desligado ou
    /// falhou, e aí não tenta de novo nesta transmissão).
    fn convert_on_gpu(&mut self, frame: &mut Frame, max_w: u32, max_h: u32) -> Option<Vec<u8>> {
        if self.gpu_failed {
            return None;
        }
        let src = (frame.width(), frame.height());
        let dst = target_size(src.0, src.1, max_w, max_h);
        let result = (|| {
            if !self.gpu.as_ref().is_some_and(|g| g.fits(src, dst)) {
                self.gpu = None;
                self.gpu = Some(GpuConverter::new(frame.device(), frame.device_context(), src, dst)?);
            }
            let gpu = self.gpu.as_ref().unwrap();
            let mut pkt = video_packet(FMT_NV12, dst.0, dst.1);
            gpu.convert(frame.device_context(), frame.as_raw_texture(), &mut pkt[HEADER..])?;
            Ok::<_, BoxError>(pkt)
        })();
        match result {
            Ok(pkt) => Some(pkt),
            Err(e) => {
                log_warn!("capture", "Could not convert frames on the GPU, using the CPU instead: {e}");
                self.gpu = None;
                self.gpu_failed = true;
                None
            }
        }
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
        Ok(c) => {
            log_info!(
                "audio",
                "Capturing audio {}",
                if include_tree { "only from the shared app" } else { "from the whole PC except AbreTela" }
            );
            c
        }
        Err(e) => {
            // Windows antigo sem process loopback: cai para o áudio do sistema todo
            log_warn!(
                "audio",
                "This Windows can't capture audio from a single app ({e}). Falling back to all PC audio,                  so viewers may also hear other sounds from this PC"
            );
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

pub fn start(id: &str, height: u32, fps: u32, audio: bool, gpu: bool) -> Result<String, String> {
    stop();
    let port = *PORT.get().ok_or("servidor de captura não iniciado")?;
    let target = parse_target(id).ok_or("fonte inválida")?;
    let fps = fps.clamp(5, 60);
    let what = match &target {
        Target::Window(w) => format!(
            "window \"{}\" ({})",
            w.title().unwrap_or_default(),
            w.process_name().unwrap_or_default()
        ),
        Target::Monitor(m) => format!("screen {}x{}", m.width().unwrap_or(0), m.height().unwrap_or(0)),
    };
    log_info!(
        "capture",
        "Starting capture of {what} at up to {height}p {fps}fps, audio {}, hardware acceleration {}",
        if audio { "on" } else { "off" },
        if gpu { "on" } else { "off" }
    );

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
            gpu,
            latest: latest.clone(),
            fresh: fresh.clone(),
            stop: stop.clone(),
        },
    )
    .inspect_err(|e| log_error!("capture", "Windows refused to start the capture: {e}"))?;

    {
        let stop = stop.clone();
        thread::spawn(move || pacer(fps, latest, fresh, stop, video_tx));
    }
    match audio_target {
        Some((pid, include_tree)) if audio => {
            let stop = stop.clone();
            thread::spawn(move || {
                if let Err(e) = audio_loop(pid, include_tree, stop, audio_tx) {
                    log_error!("audio", "Audio capture failed: {e}");
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
        log_info!("capture", "Capture stopped");
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

        let gpu = std::env::var("CAPTURE_GPU").map_or(true, |v| v != "0");
        let url = start(&screen, height, fps, true, gpu).unwrap();
        println!("url: {url}");
        let (mut video, mut audio) = SESSION.lock().unwrap().as_mut().unwrap().channels.take().unwrap();
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
            if fmt == FMT_NV12 { "NV12/GPU" } else { "I420/CPU" }
        );
        if let (Ok(path), Some(pkt)) = (std::env::var("CAPTURE_DUMP"), last) {
            dump_jpeg(&pkt, &path);
            println!("frame salvo em {path}");
        }
        assert!(nv > 0);
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
                let (u, v) = if pkt[1] == FMT_NV12 {
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
