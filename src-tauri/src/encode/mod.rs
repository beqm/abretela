//! Codificação de vídeo para o envio nativo (ROADMAP, seção 2).
//!
//! Contrato [`VideoEncoder`]: recebe frames da captura (NV12 ou I420, o mesmo
//! [`VideoPacket`] que a captura já produz) e devolve H.264 em Annex B
//! (`00 00 00 01` antes de cada NAL), pronto para mandar por WebRTC. Cada
//! sistema tem a própria implementação: no Windows, Media Foundation (a placa
//! de vídeo pelo NVENC/AMF/QuickSync, com o codificador do próprio Windows no
//! processador como reserva).
//!
//! A codificação é em fila: `encode` só entrega o frame e volta; cada frame
//! codificado sai por [`OnEncoded`] assim que fica pronto (na placa de vídeo,
//! por outra thread). Assim a placa codifica um frame enquanto o próximo
//! chega, em vez de esperar um sair para o outro entrar.

use crate::capture::{contracts::BoxError, packet::VideoPacket};
use std::{sync::Arc, time::Duration};

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

/// Recebe cada frame codificado, na ordem, assim que fica pronto. Pode ser
/// chamado de outra thread.
pub type OnEncoded = Arc<dyn Fn(EncodedFrame) + Send + Sync>;

pub trait VideoEncoder: Send {
    /// Para o log, ex.: "NVIDIA H.264 Encoder MFT (graphics card)".
    fn describe(&self) -> String;
    /// Codifica pela placa de vídeo?
    fn hardware(&self) -> bool;
    /// Largura e altura para as quais foi aberto.
    fn size(&self) -> (u32, u32);
    /// Entrega um frame (o resultado sai por [`OnEncoded`]). `keyframe` pede um
    /// quadro-chave. Se o codificador ainda estiver ocupado, espera um pouco e
    /// depois descarta este frame.
    fn encode(&mut self, frame: &VideoPacket, timestamp: Duration, keyframe: bool) -> Result<(), BoxError>;
    /// Muda o bitrate sem reiniciar.
    fn set_bitrate(&mut self, bps: u32);
    /// Para o log: onde o tempo foi desde a última chamada (zera a conta).
    fn take_timings(&mut self) -> Option<String> {
        None
    }
}

/// O melhor codificador disponível neste PC.
pub fn open(config: EncoderConfig, on_encoded: OnEncoded) -> Result<Box<dyn VideoEncoder>, BoxError> {
    Ok(Box::new(win::MfEncoder::open(config, on_encoded)?))
}

/// O frame (Annex B) tem um quadro IDR (NAL tipo 5)?
pub fn has_idr(annex_b: &[u8]) -> bool {
    annex_b.windows(4).any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] & 0x1f == 5)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::capture::packet::PixelFormat;
    use std::time::Instant;

    /// Barras que andam, em NV12.
    pub(crate) fn pattern(w: u32, h: u32, n: u32) -> VideoPacket {
        let mut pkt = VideoPacket::new(PixelFormat::Nv12, w, h);
        let (w, h) = (w as usize, h as usize);
        let px = pkt.pixels_mut();
        let (y, uv) = px.split_at_mut(w * h);
        // ENCODE_NOISE=1: ruído que muda todo frame (o pior caso, como vídeo)
        let noise = std::env::var("ENCODE_NOISE").is_ok();
        let mut seed = 0x9e37_79b9u32.wrapping_mul(n + 1);
        for row in 0..h {
            for col in 0..w {
                y[row * w + col] = if noise {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed >> 24) as u8
                } else {
                    (((col + n as usize * 8) / 64) % 8 * 28 + 16) as u8
                };
            }
        }
        for (i, v) in uv.iter_mut().enumerate() {
            *v = if i % 2 == 0 { 100 } else { 160 };
        }
        pkt
    }

    fn run(software_only: bool) {
        use std::sync::Mutex;
        let (w, h, fps) = (1920, 1080, 60);
        let config = EncoderConfig { width: w, height: h, fps, bitrate: 6_000_000, software_only };
        // quando cada frame entrou, para medir quanto demorou para sair
        let started = Instant::now();
        let out: Arc<Mutex<Vec<(EncodedFrame, Duration)>>> = Arc::default();
        let sink = out.clone();
        let on_encoded: OnEncoded = Arc::new(move |f| {
            let latency = started.elapsed().saturating_sub(f.timestamp);
            sink.lock().unwrap().push((f, latency));
        });
        let mut enc = open(config, on_encoded).expect("an encoder");
        println!("encoder: {} (hardware: {})", enc.describe(), enc.hardware());
        let frames = 120;
        for n in 0..frames {
            // no ritmo da captura (60 fps)
            let due = Duration::from_secs(1) * n / fps;
            std::thread::sleep(due.saturating_sub(started.elapsed()));
            let pkt = pattern(w, h, n);
            enc.encode(&pkt, started.elapsed(), n == 60).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));
        println!("{}", enc.take_timings().unwrap_or_default());
        drop(enc);
        let out = std::mem::take(&mut *out.lock().unwrap());
        let mut latencies: Vec<Duration> = out.iter().map(|(_, l)| *l).collect();
        latencies.sort();
        let bytes: usize = out.iter().map(|(f, _)| f.data.len()).sum();
        let median = latencies[latencies.len() / 2];
        println!(
            "{} frames in, {} out, {} keyframes, {} KB, from frame in to frame out: median {:?}, worst {:?}",
            frames,
            out.len(),
            out.iter().filter(|(f, _)| f.keyframe).count(),
            bytes / 1024,
            median,
            latencies[latencies.len() - 1],
        );
        if let Ok(path) = std::env::var("ENCODE_DUMP") {
            let all: Vec<u8> = out.iter().flat_map(|(f, _)| f.data.clone()).collect();
            std::fs::write(&path, all).unwrap();
            println!("saved to {path} (play with: ffplay {path})");
        }
        let out: Vec<EncodedFrame> = out.into_iter().map(|(f, _)| f).collect();
        assert!(out.len() >= frames as usize - 5, "most frames come out");
        assert!(median < Duration::from_millis(40), "low latency: median {median:?}");
        assert!(out[0].keyframe && has_idr(&out[0].data), "starts with a keyframe");
        assert!(out[0].data.starts_with(&[0, 0, 0, 1]) || out[0].data.starts_with(&[0, 0, 1]), "Annex B");
        assert!(out.iter().skip(1).any(|f| f.keyframe), "the requested keyframe came out");
        assert!(out.windows(2).all(|p| p[0].timestamp <= p[1].timestamp), "in order");
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
