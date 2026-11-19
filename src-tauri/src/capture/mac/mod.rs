//! Captura no macOS: telas e janelas com o ScreenCaptureKit.
//!
//! O próprio sistema reduz e converte na placa: com a aceleração ligada, os
//! frames já chegam em NV12 (BT.709, faixa limitada) no tamanho final e só são
//! copiados para o pacote. Com ela desligada, chegam em BGRA no tamanho da
//! fonte e a redução e a conversão para I420 são feitas no processador
//! (`convert.rs`), como no Windows.
//!
//! O sistema só entrega frame quando a imagem muda, igual ao Windows Graphics
//! Capture.

mod audio;

pub use audio::SckAudio;

use super::contracts::{AudioTarget, BoxError, FrameSink, RunningVideo, Source, SourceInfo, VideoConfig, VideoSource};
use super::convert::{bgra_to_i420, target_size, BgraScaler};
use super::packet::{PixelFormat, VideoPacket};
use base64::Engine;
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
    AllocAnyThread, DefinedClass,
};
use objc2_core_graphics::kCGDisplayStreamYCbCrMatrix_ITU_R_709_2;
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVPixelBuffer,
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCScreenshotManager, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamDelegate, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType, SCWindow,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

/// Quanto esperar o sistema responder (lista de fontes, início da captura).
const SYSTEM_TIMEOUT: Duration = Duration::from_secs(5);

/// Baixa a prioridade da thread atual, para a captura não disputar o
/// processador com o que está sendo transmitido.
pub fn lower_thread_priority() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// Para mandar objetos do ScreenCaptureKit a outra thread.
///
/// SAFETY de quem usa: o sistema entrega esses objetos por threads dele e
/// aceita chamadas de qualquer uma; aqui cada um é usado por uma thread de
/// cada vez.
struct Sendable<T>(T);
unsafe impl<T> Send for Sendable<T> {}

fn describe(error: *mut NSError) -> String {
    match unsafe { error.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "no details".into(),
    }
}

// ---------------------------------------------------------------------------
// Fontes (telas e janelas)
// ---------------------------------------------------------------------------

/// O que dá para capturar agora. Falha se o app não tiver a permissão de
/// gravação de tela (na primeira vez o sistema pergunta).
fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        let result = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(Sendable(content)),
            None => Err(describe(error)),
        };
        let _ = tx.send(result);
    });
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &done,
        );
    }
    match rx.recv_timeout(SYSTEM_TIMEOUT) {
        Ok(result) => result.map(|content| content.0),
        Err(_) => Err("the system did not answer".into()),
    }
}

enum Target {
    Display(Retained<SCDisplay>),
    Window(Retained<SCWindow>),
}

impl Target {
    /// `m:<id da tela>` ou `w:<id da janela>`, como em [`Source::id`].
    fn find(id: &str) -> Option<Self> {
        let (kind, raw) = id.split_once(':')?;
        let raw: u32 = raw.parse().ok()?;
        let content = shareable_content().ok()?;
        unsafe {
            match kind {
                "m" => content.displays().iter().find(|d| d.displayID() == raw).map(Target::Display),
                "w" => content.windows().iter().find(|w| w.windowID() == raw).map(Target::Window),
                _ => None,
            }
        }
    }

    fn filter(&self) -> Retained<SCContentFilter> {
        unsafe {
            match self {
                Target::Display(display) => {
                    SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, &NSArray::new())
                }
                Target::Window(window) => {
                    SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), window)
                }
            }
        }
    }
}

/// Tamanho em pixels do que o filtro captura (o sistema fala em pontos; numa
/// tela Retina cada ponto tem mais de um pixel).
fn pixel_size(filter: &SCContentFilter) -> (u32, u32) {
    let (rect, scale) = unsafe {
        let info = SCShareableContent::infoForFilter(filter);
        (info.contentRect(), info.pointPixelScale() as f64)
    };
    let pixels = |points: f64| ((points * scale).round() as u32).max(2);
    (pixels(rect.size.width), pixels(rect.size.height))
}

/// Como o sistema deve entregar os frames: tamanho, formato e ritmo.
fn configuration(size: (u32, u32), format: u32, fps: Option<u32>) -> Retained<SCStreamConfiguration> {
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth(size.0 as usize);
        config.setHeight(size.1 as usize);
        config.setPixelFormat(format);
        config.setShowsCursor(true);
        if format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
            config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
        }
        if let Some(fps) = fps {
            config.setMinimumFrameInterval(CMTime::new(1, fps as i32));
            config.setQueueDepth(3);
        }
        config
    }
}

fn window_title(window: &SCWindow) -> String {
    unsafe { window.title() }.map(|t| t.to_string().trim().to_string()).unwrap_or_default()
}

pub struct SckSource;

impl VideoSource for SckSource {
    fn list(&self) -> Vec<Source> {
        let content = match shareable_content() {
            Ok(content) => content,
            Err(e) => {
                log_warn!("capture", "Could not list the screens and windows (is screen recording allowed for AbreTela?): {e}");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        unsafe {
            for (i, display) in content.displays().iter().enumerate() {
                let (w, h) = pixel_size(&Target::Display(display.clone()).filter());
                out.push(Source {
                    id: format!("m:{}", display.displayID()),
                    kind: "screen",
                    title: (i + 1).to_string(), // o frontend traduz ("Tela 1")
                    app: format!("{w}×{h}"),
                });
            }
            let me = std::process::id() as i32;
            for window in content.windows().iter() {
                // camada 0 = janelas comuns (fora menus, Dock, barra de menus)
                let Some(app) = window.owningApplication().filter(|_| window.windowLayer() == 0) else { continue };
                let title = window_title(&window);
                if title.is_empty() || app.processID() == me {
                    continue;
                }
                out.push(Source {
                    id: format!("w:{}", window.windowID()),
                    kind: "window",
                    title,
                    app: app.applicationName().to_string(),
                });
            }
        }
        out
    }

    fn thumbnail(&self, id: &str) -> Option<String> {
        let filter = Target::find(id)?.filter();
        let (w, h) = pixel_size(&filter);
        let config = configuration(target_size(w, h, 400, 225), kCVPixelFormatType_32BGRA, None);
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |sample: *mut CMSampleBuffer, _error: *mut NSError| {
            let _ = tx.send(unsafe { sample.as_ref() }.and_then(jpeg_data_url));
        });
        unsafe {
            SCScreenshotManager::captureSampleBufferWithFilter_configuration_completionHandler(&filter, &config, Some(&done));
        }
        rx.recv_timeout(Duration::from_millis(1500)).ok().flatten()
    }

    fn inspect(&self, id: &str) -> Option<SourceInfo> {
        Some(match Target::find(id)? {
            Target::Window(window) => {
                let app = unsafe { window.owningApplication() };
                SourceInfo {
                    description: format!(
                        "window \"{}\" ({})",
                        window_title(&window),
                        app.as_ref().map(|a| unsafe { a.applicationName() }.to_string()).unwrap_or_default()
                    ),
                    audio: app.map(|a| AudioTarget { pid: unsafe { a.processID() } as u32, include_tree: true }),
                }
            }
            Target::Display(display) => {
                let (w, h) = pixel_size(&Target::Display(display).filter());
                SourceInfo {
                    description: format!("screen {w}x{h}"),
                    // tela inteira: tudo, menos o som deste app (as streams dos amigos)
                    audio: Some(AudioTarget { pid: std::process::id(), include_tree: false }),
                }
            }
        })
    }

    fn start(
        &self,
        id: &str,
        config: VideoConfig,
        sink: Arc<dyn FrameSink>,
        stop: Arc<AtomicBool>,
    ) -> Result<RunningVideo, String> {
        let filter = Target::find(id).ok_or("fonte inválida")?.filter();
        let source = pixel_size(&filter);
        let max_h = config.max_height;
        let max_w = max_h * 21 / 9; // limita ultrawide
        let stream_config = if config.gpu {
            let size = target_size(source.0, source.1, max_w, max_h);
            configuration(size, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, Some(config.fps))
        } else {
            configuration(source, kCVPixelFormatType_32BGRA, Some(config.fps))
        };

        let output = Output::new(OutputState {
            sink,
            stop,
            source,
            max: (max_w, max_h),
            frames: Mutex::default(),
        });
        let queue = DispatchQueue::new("abretela.capture", DispatchQueueAttr::SERIAL);
        let stream = unsafe {
            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &stream_config,
                Some(ProtocolObject::from_ref(&*output)),
            );
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| format!("falha ao iniciar a captura: {}", e.localizedDescription()))?;
            stream
        };

        let (tx, rx) = mpsc::channel();
        let started = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(if error.is_null() { Ok(()) } else { Err(describe(error)) });
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
        match rx.recv_timeout(SYSTEM_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("falha ao iniciar a captura: {e}")),
            Err(_) => return Err("falha ao iniciar a captura: o sistema não respondeu".into()),
        }

        let running = Sendable((stream, output, queue));
        Ok(RunningVideo {
            stop: Box::new(move || {
                let running = Arc::new(running);
                let keep = running.clone();
                // a captura, o destino dos frames e a fila só são soltos
                // depois de o sistema terminar de parar
                let stopped = RcBlock::new(move |_error: *mut NSError| {
                    let _ = &keep;
                });
                unsafe { running.0 .0.stopCaptureWithCompletionHandler(Some(&stopped)) };
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// Pixels de um frame do sistema
// ---------------------------------------------------------------------------

/// Um buffer do sistema travado para leitura (destrava ao sair de escopo).
struct Locked<'a>(&'a CVPixelBuffer);

impl<'a> Locked<'a> {
    fn new(buffer: &'a CVPixelBuffer) -> Result<Self, BoxError> {
        let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status != 0 {
            return Err(format!("could not read the captured frame (error {status})").into());
        }
        Ok(Self(buffer))
    }

    fn size(&self) -> (usize, usize) {
        (CVPixelBufferGetWidth(self.0), CVPixelBufferGetHeight(self.0))
    }

    /// As linhas do plano `index` (cada uma com sobra no fim: o `stride`).
    fn plane(&self, index: usize, rows: usize) -> std::slice::ChunksExact<'_, u8> {
        unsafe {
            let stride = CVPixelBufferGetBytesPerRowOfPlane(self.0, index);
            let base = CVPixelBufferGetBaseAddressOfPlane(self.0, index) as *const u8;
            std::slice::from_raw_parts(base, stride * rows).chunks_exact(stride)
        }
    }

    /// Os pixels BGRA sem a sobra das linhas, em `out`.
    fn bgra(&self, out: &mut Vec<u8>) {
        let (w, h) = self.size();
        out.clear();
        unsafe {
            let stride = CVPixelBufferGetBytesPerRow(self.0);
            let base = CVPixelBufferGetBaseAddress(self.0) as *const u8;
            for row in std::slice::from_raw_parts(base, stride * h).chunks_exact(stride) {
                out.extend_from_slice(&row[..w * 4]);
            }
        }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly);
        }
    }
}

/// Uma imagem pequena (data URL JPEG) de uma amostra BGRA.
fn jpeg_data_url(sample: &CMSampleBuffer) -> Option<String> {
    let image = unsafe { sample.image_buffer() }?;
    let pixels = Locked::new(&image).ok()?;
    let (w, h) = pixels.size();
    let mut bgra = Vec::new();
    pixels.bgra(&mut bgra);
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 75).encode(&bgra, w as u16, h as u16, jpeg_encoder::ColorType::Bgra).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
    Some(format!("data:image/jpeg;base64,{b64}"))
}

/// O sistema diz o que é cada amostra: só as completas têm imagem nova
/// (as outras avisam que a tela não mudou, que a captura começou etc.).
fn is_complete(sample: &CMSampleBuffer) -> bool {
    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else { return false };
    // a lista do CoreFoundation é a mesma coisa que um NSArray de NSDictionary
    let attachments = unsafe { &*((&*attachments) as *const _ as *const NSArray<NSDictionary<NSString, AnyObject>>) };
    let Some(info) = attachments.firstObject() else { return false };
    let status = unsafe { info.objectForKey(SCStreamFrameInfoStatus) };
    status.and_then(|s| s.downcast::<NSNumber>().ok()).map(|n| n.integerValue()) == Some(SCFrameStatus::Complete.0)
}

// ---------------------------------------------------------------------------
// Transmissão
// ---------------------------------------------------------------------------

struct OutputState {
    sink: Arc<dyn FrameSink>,
    stop: Arc<AtomicBool>,
    /// Tamanho da fonte em pixels (para o log).
    source: (u32, u32),
    /// Maior tamanho a enviar.
    max: (u32, u32),
    frames: Mutex<Frames>,
}

/// O que muda a cada frame (o sistema chama numa fila só, um por vez).
struct Frames {
    count: u64,
    scaler: BgraScaler,
    scratch: Vec<u8>,
    /// Tempo gasto processando frames desde o último relatório no log.
    busy: Duration,
    busy_frames: u32,
    next_report: Instant,
}

impl Default for Frames {
    fn default() -> Self {
        Self {
            count: 0,
            scaler: BgraScaler::default(),
            scratch: Vec::new(),
            busy: Duration::ZERO,
            busy_frames: 0,
            next_report: Instant::now() + Duration::from_secs(10),
        }
    }
}

impl OutputState {
    fn packet(&self, image: &CVPixelBuffer, frames: &mut Frames) -> Result<VideoPacket, BoxError> {
        let pixels = Locked::new(image)?;
        let (w, h) = pixels.size();
        if CVPixelBufferGetPixelFormatType(image) == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
            // já no tamanho final (par, por `target_size`): só copia os dois planos
            let (w, h) = (w & !1, h & !1);
            let mut pkt = VideoPacket::new(PixelFormat::Nv12, w as u32, h as u32);
            let (luma, chroma) = pkt.pixels_mut().split_at_mut(w * h);
            for (dst, src) in luma.chunks_exact_mut(w).zip(pixels.plane(0, h)) {
                dst.copy_from_slice(&src[..w]);
            }
            for (dst, src) in chroma.chunks_exact_mut(w).zip(pixels.plane(1, h / 2)) {
                dst.copy_from_slice(&src[..w]);
            }
            return Ok(pkt);
        }
        // no processador: reduz em BGRA e converte para I420
        pixels.bgra(&mut frames.scratch);
        let (dw, dh, bgra) = frames.scaler.scale(&frames.scratch, w as u32, h as u32, self.max.0, self.max.1)?;
        let mut pkt = VideoPacket::new(PixelFormat::I420, dw, dh);
        bgra_to_i420(dw as usize, dh as usize, &bgra, pkt.pixels_mut());
        Ok(pkt)
    }

    fn frame(&self, sample: &CMSampleBuffer) {
        if self.stop.load(Ordering::Relaxed) || !is_complete(sample) {
            return;
        }
        let Some(image) = (unsafe { sample.image_buffer() }) else { return };
        let mut frames = self.frames.lock().unwrap();
        frames.count += 1;
        let started = Instant::now();
        let pkt = match self.packet(&image, &mut frames) {
            Ok(pkt) => pkt,
            Err(e) => {
                if frames.count <= 3 {
                    log_error!("capture", "Could not process a captured frame: {e}");
                }
                return;
            }
        };
        frames.report_time(started.elapsed(), pkt.format());
        if frames.count == 1 {
            log_info!(
                "capture",
                "First frame captured: {}x{}, sent as {}x{}, converted on the {}",
                self.source.0,
                self.source.1,
                pkt.width(),
                pkt.height(),
                pkt.format().converted_on()
            );
        }
        drop(frames);
        self.sink.video(pkt);
    }
}

impl Frames {
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

define_class!(
    // SAFETY: NSObject não tem exigências para subclasses, e esta não
    // implementa `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "AbreTelaCaptureOutput"]
    #[ivars = OutputState]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen {
                self.ivars().frame(sample);
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_did_stop(&self, _stream: &SCStream, error: &NSError) {
            // janela fechada: encerra tudo (o frontend percebe pelo WebSocket)
            let state = self.ivars();
            if !state.stop.swap(true, Ordering::Relaxed) {
                log_info!(
                    "capture",
                    "The captured window or screen went away, stopping ({} frames captured): {}",
                    state.frames.lock().unwrap().count,
                    error.localizedDescription()
                );
            }
        }
    }
);

impl Output {
    fn new(state: OutputState) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}
