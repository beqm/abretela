//! Som de um app (janela) ou de tudo menos este app (tela inteira), pelo
//! ScreenCaptureKit.
//!
//! O sistema só entrega som junto de uma captura de tela, então aqui roda uma
//! captura à parte, com uma imagem mínima que é ignorada: assim o som segue o
//! mesmo contrato do Windows (`AudioSource`), separado do vídeo.

use super::{describe, shareable_content, Sendable, SYSTEM_TIMEOUT};
use crate::capture::contracts::{AudioSource, AudioTarget, BoxError, FrameSink};
use crate::capture::packet::audio_packet;
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
    AllocAnyThread, DefinedClass,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMBlockBuffer, CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// 10 ms de f32 estéreo a 48 kHz, o pedaço que o resto do app espera.
const CHUNK: usize = 480 * 8;

pub struct SckAudio;

impl AudioSource for SckAudio {
    fn run(&self, target: AudioTarget, sink: Arc<dyn FrameSink>, stop: Arc<AtomicBool>) -> Result<(), BoxError> {
        let AudioTarget { pid, include_tree } = target;
        let content = shareable_content()?;
        let filter = unsafe {
            let display = content.displays().firstObject().ok_or("no screen to capture audio from")?;
            let app = content.applications().iter().find(|a| a.processID() == pid as i32);
            let none = NSArray::new();
            match (include_tree, app) {
                // só o app da janela
                (true, Some(app)) => SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&[app]),
                    &none,
                ),
                (true, None) => return Err("the app of the shared window is gone".into()),
                // tudo menos este app (as telas dos amigos tocando aqui)
                (false, Some(me)) => SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&[me]),
                    &none,
                ),
                (false, None) => {
                    SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &none)
                }
            }
        };
        let config = unsafe {
            let config = SCStreamConfiguration::new();
            config.setCapturesAudio(true);
            config.setSampleRate(48_000);
            config.setChannelCount(2);
            config.setExcludesCurrentProcessAudio(!include_tree);
            // a imagem que vem junto não é usada: a menor e mais lenta possível
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
            config.setShowsCursor(false);
            config
        };

        let output = Output::new(OutputState { sink: sink.clone(), pending: Mutex::default(), ended: AtomicBool::new(false) });
        let queue = DispatchQueue::new("abretela.capture.audio", DispatchQueueAttr::SERIAL);
        let stream = unsafe {
            let delegate = ProtocolObject::from_ref(&*output);
            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, Some(delegate));
            // a imagem também precisa de um destino, senão o sistema reclama a cada frame
            for kind in [SCStreamOutputType::Audio, SCStreamOutputType::Screen] {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), kind, Some(&queue))
                    .map_err(|e| e.localizedDescription().to_string())?;
            }
            stream
        };
        let (tx, rx) = mpsc::channel();
        let started = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(if error.is_null() { Ok(()) } else { Err(describe(error)) });
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
        rx.recv_timeout(SYSTEM_TIMEOUT).map_err(|_| "the system did not answer")??;
        log_info!(
            "audio",
            "Capturing audio {}",
            if include_tree { "only from the shared app" } else { "from the whole Mac except AbreTela" }
        );

        while !stop.load(Ordering::Relaxed) && !sink.is_closed() && !output.ivars().ended.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }

        // a captura, o destino e a fila só são soltos depois de o sistema parar
        let running = Arc::new(Sendable((stream, output, queue)));
        let keep = running.clone();
        let stopped = RcBlock::new(move |_error: *mut NSError| {
            let _ = &keep;
        });
        unsafe { running.0 .0.stopCaptureWithCompletionHandler(Some(&stopped)) };
        Ok(())
    }
}

struct OutputState {
    sink: Arc<dyn FrameSink>,
    /// Bytes de som esperando completar um pedaço de 10 ms.
    pending: Mutex<Vec<u8>>,
    /// O sistema parou a captura (ex.: o app da janela fechou).
    ended: AtomicBool,
}

impl OutputState {
    fn audio(&self, sample: &CMSampleBuffer) {
        let Some(samples) = (unsafe { stereo_samples(sample) }) else { return };
        let mut pending = self.pending.lock().unwrap();
        pending.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        let whole = pending.len() / CHUNK * CHUNK;
        for chunk in pending[..whole].chunks_exact(CHUNK) {
            self.sink.audio(audio_packet(chunk.iter().copied()));
        }
        pending.drain(..whole);
    }
}

/// Como o CoreAudio descreve o som de uma amostra: um buffer por canal (o
/// normal aqui) ou um só com os canais intercalados.
#[repr(C)]
struct Buffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}

/// `AudioBufferList` com lugar para dois buffers (estéreo em planos separados).
#[repr(C)]
struct BufferList {
    count: u32,
    buffers: [Buffer; 2],
}

/// As amostras de som (f32) de `sample`, em estéreo intercalado.
unsafe fn stereo_samples(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
    let mut list: BufferList = std::mem::zeroed();
    let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
    let status = sample.audio_buffer_list_with_retained_block_buffer(
        std::ptr::null_mut(),
        (&mut list as *mut BufferList).cast(),
        std::mem::size_of::<BufferList>(),
        None,
        None,
        0,
        &mut block,
    );
    // o bloco segura a memória para onde os buffers apontam; solto ao sair
    let _block = std::ptr::NonNull::new(block).map(|b| CFRetained::from_raw(b));
    if status != 0 {
        return None;
    }
    let planes: Vec<(u32, &[f32])> = list.buffers[..(list.count as usize).min(2)]
        .iter()
        .filter(|b| !b.data.is_null())
        .map(|b| (b.channels, std::slice::from_raw_parts(b.data as *const f32, b.bytes as usize / 4)))
        .collect();
    Some(to_stereo(&planes))
}

/// Junta os buffers (canais, amostras) em estéreo intercalado (L, R, L, R...).
fn to_stereo(planes: &[(u32, &[f32])]) -> Vec<f32> {
    match planes {
        // já intercalado
        [(2, both)] => both.to_vec(),
        // mono: o mesmo som nos dois lados
        [(_, mono)] => mono.iter().flat_map(|&s| [s, s]).collect(),
        // um plano por canal
        [(_, left), (_, right), ..] => left.iter().zip(right.iter()).flat_map(|(&l, &r)| [l, r]).collect(),
        [] => Vec::new(),
    }
}

define_class!(
    // SAFETY: NSObject não tem exigências para subclasses, e esta não
    // implementa `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "AbreTelaAudioOutput"]
    #[ivars = OutputState]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Audio {
                self.ivars().audio(sample);
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_did_stop(&self, _stream: &SCStream, error: &NSError) {
            if !self.ivars().ended.swap(true, Ordering::Relaxed) {
                log_info!("audio", "The system stopped the audio capture: {}", error.localizedDescription());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_one_plane_per_channel() {
        let (left, right) = ([0.1, 0.2, 0.3], [-0.1, -0.2, -0.3]);
        assert_eq!(to_stereo(&[(1, &left), (1, &right)]), [0.1, -0.1, 0.2, -0.2, 0.3, -0.3]);
    }

    #[test]
    fn keeps_interleaved_stereo_and_doubles_mono() {
        assert_eq!(to_stereo(&[(2, &[0.1, -0.1, 0.2, -0.2])]), [0.1, -0.1, 0.2, -0.2]);
        assert_eq!(to_stereo(&[(1, &[0.5, 0.6])]), [0.5, 0.5, 0.6, 0.6]);
        assert!(to_stereo(&[]).is_empty());
    }
}
