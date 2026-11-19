//! Codificador H.264 do macOS (VideoToolbox).
//!
//! Ainda sem o codificador de verdade: `open` recusa, e o envio nativo avisa
//! no log que não conseguiu codificar.

use super::{EncoderConfig, OnEncoded, VideoEncoder};
use crate::capture::{contracts::BoxError, packet::VideoPacket};
use std::time::Duration;

pub struct VtEncoder;

impl VtEncoder {
    pub fn open(_config: EncoderConfig, _on_encoded: OnEncoded) -> Result<Self, BoxError> {
        Err("o codificador de vídeo ainda não existe no macOS".into())
    }
}

impl VideoEncoder for VtEncoder {
    fn describe(&self) -> String {
        "VideoToolbox H.264".into()
    }

    fn hardware(&self) -> bool {
        true
    }

    fn encode(&mut self, _frame: &VideoPacket, _timestamp: Duration, _keyframe: bool) -> Result<(), BoxError> {
        Err("o codificador de vídeo ainda não existe no macOS".into())
    }

    fn set_bitrate(&mut self, _bps: u32) {}
}
