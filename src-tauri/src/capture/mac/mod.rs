//! Captura no macOS: telas e janelas com o ScreenCaptureKit e o som pelo
//! mesmo caminho.
//!
//! Ainda sem a captura de verdade: o app abre e assiste, mas não lista fontes
//! nem transmite.

use super::contracts::{AudioSource, AudioTarget, BoxError, FrameSink, RunningVideo, Source, SourceInfo, VideoConfig, VideoSource};
use std::sync::{atomic::AtomicBool, Arc};

/// Baixa a prioridade da thread atual, para a captura não disputar o
/// processador com o que está sendo transmitido.
pub fn lower_thread_priority() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

pub struct SckSource;

impl VideoSource for SckSource {
    fn list(&self) -> Vec<Source> {
        Vec::new()
    }

    fn thumbnail(&self, _id: &str) -> Option<String> {
        None
    }

    fn inspect(&self, _id: &str) -> Option<SourceInfo> {
        None
    }

    fn start(
        &self,
        _id: &str,
        _config: VideoConfig,
        _sink: Arc<dyn FrameSink>,
        _stop: Arc<AtomicBool>,
    ) -> Result<RunningVideo, String> {
        Err("a captura de tela ainda não existe no macOS".into())
    }
}

pub struct SckAudio;

impl AudioSource for SckAudio {
    fn run(&self, _target: AudioTarget, _sink: Arc<dyn FrameSink>, _stop: Arc<AtomicBool>) -> Result<(), BoxError> {
        Err("a captura de som ainda não existe no macOS".into())
    }
}
