//! Captura no Windows: telas e janelas com o Windows Graphics Capture,
//! conversão na placa (D3D11) ou no processador e som pelo WASAPI.

mod audio;
mod gpu_convert;

pub use audio::WasapiLoopback;

use super::contracts::{
    AudioTarget, BoxError, FrameConverter, FrameSink, RunningVideo, Source, SourceInfo, VideoConfig, VideoSource,
};
use super::convert::{bgra_to_i420, target_size, BgraScaler};
use super::packet::{PixelFormat, VideoPacket};
use base64::Engine;
use gpu_convert::GpuConverter;
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings,
        SecondaryWindowSettings, Settings,
    },
    window::Window,
};

/// Baixa a prioridade da thread atual, para a captura não disputar o
/// processador com o jogo.
pub fn lower_thread_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

// ---------------------------------------------------------------------------
// Fontes (telas e janelas)
// ---------------------------------------------------------------------------

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

/// Inicia uma sessão WGC (janela ou monitor) com o handler `H`.
fn start_session<H>(
    target: Target,
    fps: Option<u32>,
    flags: H::Flags,
) -> Result<windows_capture::capture::CaptureControl<H, BoxError>, String>
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

/// Reduz os pixels BGRA do frame (lidos sem o padding das linhas, sem cópia
/// extra) para caber em `max_w` x `max_h`: (largura, altura, bgra).
fn scaled_bgra(
    frame: &mut Frame,
    max_w: u32,
    max_h: u32,
    scaler: &mut BgraScaler,
    scratch: &mut Vec<u8>,
) -> Result<(u32, u32, Vec<u8>), BoxError> {
    let mut buf = frame.buffer()?;
    let (w, h) = (buf.width(), buf.height());
    let src: &[u8] = if buf.has_padding() { buf.as_nopadding_buffer(scratch) } else { buf.as_raw_buffer() };
    scaler.scale(src, w, h, max_w, max_h)
}

pub struct WgcSource;

impl VideoSource for WgcSource {
    fn list(&self) -> Vec<Source> {
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

    fn thumbnail(&self, id: &str) -> Option<String> {
        let target = parse_target(id)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let control = start_session::<Thumb>(target, None, tx).ok()?;
        let result = rx.recv_timeout(Duration::from_millis(1500)).ok();
        let _ = control.stop();
        result
    }

    fn inspect(&self, id: &str) -> Option<SourceInfo> {
        Some(match parse_target(id)? {
            Target::Window(w) => SourceInfo {
                description: format!(
                    "window \"{}\" ({})",
                    w.title().unwrap_or_default(),
                    w.process_name().unwrap_or_default()
                ),
                audio: w.process_id().ok().map(|pid| AudioTarget { pid, include_tree: true }),
            },
            Target::Monitor(m) => SourceInfo {
                description: format!("screen {}x{}", m.width().unwrap_or(0), m.height().unwrap_or(0)),
                // tela inteira: tudo, menos o som deste app (as streams dos amigos)
                audio: Some(AudioTarget { pid: std::process::id(), include_tree: false }),
            },
        })
    }

    fn start(
        &self,
        id: &str,
        config: VideoConfig,
        sink: Arc<dyn FrameSink>,
        stop: Arc<AtomicBool>,
    ) -> Result<RunningVideo, String> {
        let target = parse_target(id).ok_or("fonte inválida")?;
        let control = start_session::<Streamer>(target, Some(config.fps), StreamFlags { config, sink, stop })?;
        Ok(RunningVideo {
            stop: Box::new(move || {
                let _ = control.stop();
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// Conversores
// ---------------------------------------------------------------------------

/// No processador: reduz em BGRA e converte para I420.
#[derive(Default)]
struct CpuI420 {
    scaler: BgraScaler,
    scratch: Vec<u8>,
}

impl FrameConverter<Frame<'_>> for CpuI420 {
    fn convert(&mut self, frame: &mut Frame, max_w: u32, max_h: u32) -> Result<VideoPacket, BoxError> {
        let (w, h, bgra) = scaled_bgra(frame, max_w, max_h, &mut self.scaler, &mut self.scratch)?;
        let mut pkt = VideoPacket::new(PixelFormat::I420, w, h);
        bgra_to_i420(w as usize, h as usize, &bgra, pkt.pixels_mut());
        Ok(pkt)
    }
}

/// Na placa de vídeo: o processador de vídeo do D3D11 reduz e converte para NV12.
#[derive(Default)]
struct GpuNv12 {
    converter: Option<GpuConverter>,
}

impl FrameConverter<Frame<'_>> for GpuNv12 {
    fn convert(&mut self, frame: &mut Frame, max_w: u32, max_h: u32) -> Result<VideoPacket, BoxError> {
        let src = (frame.width(), frame.height());
        let dst = target_size(src.0, src.1, max_w, max_h);
        if !self.converter.as_ref().is_some_and(|g| g.fits(src, dst)) {
            self.converter = None;
            self.converter = Some(GpuConverter::new(frame.device(), frame.device_context(), src, dst)?);
        }
        let converter = self.converter.as_ref().unwrap();
        let mut pkt = VideoPacket::new(PixelFormat::Nv12, dst.0, dst.1);
        converter.convert(frame.device_context(), frame.as_raw_texture(), pkt.pixels_mut())?;
        Ok(pkt)
    }
}

// ---------------------------------------------------------------------------
// Miniaturas
// ---------------------------------------------------------------------------

struct Thumb {
    tx: Option<std::sync::mpsc::Sender<String>>,
    scaler: BgraScaler,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Thumb {
    type Flags = std::sync::mpsc::Sender<String>;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { tx: Some(ctx.flags), scaler: BgraScaler::default(), scratch: Vec::new() })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, control: InternalCaptureControl) -> Result<(), Self::Error> {
        if let Some(tx) = self.tx.take() {
            let (w, h, bgra) = scaled_bgra(frame, 400, 225, &mut self.scaler, &mut self.scratch)?;
            let mut jpeg = Vec::new();
            jpeg_encoder::Encoder::new(&mut jpeg, 75).encode(&bgra, w as u16, h as u16, jpeg_encoder::ColorType::Bgra)?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
            let _ = tx.send(format!("data:image/jpeg;base64,{b64}"));
        }
        control.stop();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Transmissão
// ---------------------------------------------------------------------------

struct StreamFlags {
    config: VideoConfig,
    sink: Arc<dyn FrameSink>,
    stop: Arc<AtomicBool>,
}

struct Streamer {
    flags: StreamFlags,
    cpu: CpuI420,
    /// `None` = aceleração desligada, ou a conversão na placa falhou uma vez
    /// e segue no processador até o fim desta transmissão.
    gpu: Option<GpuNv12>,
    frames: u64,
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
            gpu: ctx.flags.config.gpu.then(GpuNv12::default),
            flags: ctx.flags,
            cpu: CpuI420::default(),
            frames: 0,
            busy: Duration::ZERO,
            busy_frames: 0,
            next_report: Instant::now() + Duration::from_secs(10),
        })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, control: InternalCaptureControl) -> Result<(), Self::Error> {
        self.frames += 1;
        if self.flags.stop.load(Ordering::Relaxed) {
            control.stop();
            return Ok(());
        }
        if self.frames == 1 {
            lower_thread_priority();
        }
        let max_h = self.flags.config.max_height;
        let max_w = max_h * 21 / 9; // limita ultrawide
        let src = (frame.width(), frame.height());
        let started = Instant::now();
        let gpu_result = self.gpu.as_mut().map(|gpu| gpu.convert(frame, max_w, max_h));
        let pkt = match gpu_result {
            Some(Ok(pkt)) => pkt,
            other => {
                if let Some(Err(e)) = other {
                    log_warn!("capture", "Could not convert frames on the GPU, using the CPU instead: {e}");
                    self.gpu = None;
                }
                self.cpu
                    .convert(frame, max_w, max_h)
                    .inspect_err(|e| log_error!("capture", "Could not process a captured frame: {e}"))?
            }
        };
        self.report_time(started.elapsed(), pkt.format());
        if self.frames == 1 {
            log_info!(
                "capture",
                "First frame captured: {}x{}, sent as {}x{}, converted on the {}",
                src.0,
                src.1,
                pkt.width(),
                pkt.height(),
                pkt.format().converted_on()
            );
        }
        self.flags.sink.video(pkt);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        // janela fechada: encerra tudo (o frontend percebe pelo WebSocket)
        log_info!("capture", "The captured window or screen went away, stopping ({} frames captured)", self.frames);
        self.flags.stop.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Streamer {
    /// De tempos em tempos, diz no log quanto custa processar cada frame.
    fn report_time(&mut self, took: Duration, format: PixelFormat) {
        self.busy += took;
        self.busy_frames += 1;
        if Instant::now() < self.next_report {
            return;
        }
        log_info!(
            "capture",
            "Processing a frame takes {:.1} ms on average on the {} ({} frames)",
            self.busy.as_secs_f64() * 1000.0 / self.busy_frames as f64,
            format.converted_on(),
            self.busy_frames
        );
        self.busy = Duration::ZERO;
        self.busy_frames = 0;
        self.next_report = Instant::now() + Duration::from_secs(300);
    }
}
