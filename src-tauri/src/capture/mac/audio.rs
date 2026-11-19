//! Som de um app (janela) ou de tudo menos este app (tela inteira), pelo
//! ScreenCaptureKit.
//!
//! Ainda por fazer: por enquanto a transmissão sai sem som.

use crate::capture::contracts::{AudioSource, AudioTarget, BoxError, FrameSink};
use std::sync::{atomic::AtomicBool, Arc};

pub struct SckAudio;

impl AudioSource for SckAudio {
    fn run(&self, _target: AudioTarget, _sink: Arc<dyn FrameSink>, _stop: Arc<AtomicBool>) -> Result<(), BoxError> {
        Err("a captura de som ainda não existe no macOS".into())
    }
}
