//! Codificação de vídeo para o envio nativo (ROADMAP, seção 2).
//!
//! Contrato [`VideoEncoder`]: recebe frames da captura (NV12 ou I420, o mesmo
//! [`VideoPacket`] que a captura já produz) e devolve H.264 em Annex B
//! (`00 00 00 01` antes de cada NAL), pronto para mandar por WebRTC. Cada
//! sistema tem a própria implementação: no Windows, Media Foundation (a placa
//! de vídeo pelo NVENC/AMF/QuickSync, com o codificador do próprio Windows no
//! processador como reserva).

// ainda sem uso no app: o transporte nativo (ROADMAP 3b) é quem vai usar
#![allow(dead_code)]

use crate::capture::{contracts::BoxError, packet::VideoPacket};
use std::time::Duration;

mod win;

/// Como codificar.
#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Bits por segundo pretendidos (o controle de banda muda depois).
    pub bitrate: u32,
    /// Só usar o processador (para testar a reserva).
    pub software_only: bool,
}

/// Um frame codificado.
#[derive(Debug)]
pub struct EncodedFrame {
    /// H.264 Annex B.
    pub data: Vec<u8>,
    /// Quadro-chave (IDR): dá para começar a decodificar por ele.
    pub keyframe: bool,
    /// O mesmo tempo que entrou com o frame.
    pub timestamp: Duration,
}

pub trait VideoEncoder: Send {
    /// Para o log, ex.: "NVIDIA H.264 Encoder MFT (graphics card)".
    fn describe(&self) -> String;
    /// Codifica pela placa de vídeo?
    fn hardware(&self) -> bool;
    /// Entrega um frame; devolve os frames codificados que ficaram prontos
    /// (normalmente um, às vezes nenhum se o codificador segurou um pouco).
    /// `keyframe` pede um quadro-chave.
    fn encode(&mut self, frame: &VideoPacket, timestamp: Duration, keyframe: bool) -> Result<Vec<EncodedFrame>, BoxError>;
    /// Muda o bitrate sem reiniciar.
    fn set_bitrate(&mut self, bps: u32);
}

/// O melhor codificador disponível neste PC.
pub fn open(config: EncoderConfig) -> Result<Box<dyn VideoEncoder>, BoxError> {
    Ok(Box::new(win::MfEncoder::open(config)?))
}

/// O frame (Annex B) tem um quadro IDR (NAL tipo 5)?
pub fn has_idr(annex_b: &[u8]) -> bool {
    annex_b.windows(4).any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] & 0x1f == 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::packet::PixelFormat;
    use std::time::Instant;

    /// Barras que andam, em NV12.
    fn pattern(w: u32, h: u32, n: u32) -> VideoPacket {
        let mut pkt = VideoPacket::new(PixelFormat::Nv12, w, h);
        let (w, h) = (w as usize, h as usize);
        let px = pkt.pixels_mut();
        let (y, uv) = px.split_at_mut(w * h);
        for row in 0..h {
            for col in 0..w {
                y[row * w + col] = (((col + n as usize * 8) / 64) % 8 * 28 + 16) as u8;
            }
        }
        for (i, v) in uv.iter_mut().enumerate() {
            *v = if i % 2 == 0 { 100 } else { 160 };
        }
        pkt
    }

    fn run(software_only: bool) {
        let (w, h, fps) = (1920, 1080, 60);
        let config = EncoderConfig { width: w, height: h, fps, bitrate: 6_000_000, software_only };
        let mut enc = open(config).expect("an encoder");
        println!("encoder: {} (hardware: {})", enc.describe(), enc.hardware());
        let frames = 120;
        let mut out = Vec::new();
        let mut times = Vec::new();
        let mut max_delay = 0;
        for n in 0..frames {
            let pkt = pattern(w, h, n);
            let t = Instant::now();
            let ts = Duration::from_secs(1) * n / fps;
            let ready = enc.encode(&pkt, ts, n == 60).unwrap();
            times.push(t.elapsed());
            // quantos frames o codificador segurou (atraso)
            for f in &ready {
                let index = (f.timestamp.as_secs_f64() * fps as f64).round() as u32;
                max_delay = max_delay.max(n - index);
            }
            out.extend(ready);
        }
        times.sort();
        let bytes: usize = out.iter().map(|f| f.data.len()).sum();
        println!(
            "{} frames in, {} out, {} keyframes, {} KB, encode time median {:?}, worst {:?}, held up to {max_delay} frames",
            frames,
            out.len(),
            out.iter().filter(|f| f.keyframe).count(),
            bytes / 1024,
            times[times.len() / 2],
            times[times.len() - 1],
        );
        if let Ok(path) = std::env::var("ENCODE_DUMP") {
            let all: Vec<u8> = out.iter().flat_map(|f| f.data.clone()).collect();
            std::fs::write(&path, all).unwrap();
            println!("saved to {path} (play with: ffplay {path})");
        }
        assert!(out.len() >= frames as usize - 5, "most frames come out");
        assert!(max_delay <= 1, "low latency: held {max_delay} frames");
        assert!(out[0].keyframe && has_idr(&out[0].data), "starts with a keyframe");
        assert!(out[0].data.starts_with(&[0, 0, 0, 1]) || out[0].data.starts_with(&[0, 0, 1]), "Annex B");
        assert!(out.iter().skip(1).any(|f| f.keyframe), "the requested keyframe came out");
    }

    /// `cargo test encode_hardware -- --ignored --nocapture`
    /// (precisa de Media Foundation e de uma placa de vídeo com codificador)
    #[test]
    #[ignore]
    fn encode_hardware() {
        run(false);
    }

    /// `cargo test encode_software -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn encode_software() {
        run(true);
    }

    #[test]
    fn finds_idr() {
        assert!(has_idr(&[0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x65, 9]));
        assert!(!has_idr(&[0, 0, 0, 1, 0x41, 9, 9]));
    }
}
