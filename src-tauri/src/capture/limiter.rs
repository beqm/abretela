//! Limitador do som capturado. A captura do som de um app só (e às vezes a do
//! sistema inteiro) pega o som antes do volume e do limitador do próprio
//! sistema: um jogo alto entrega amostras acima de 1,0 em float, que quem
//! transmite ouve limpas, mas que, para quem assiste, são cortadas no 1,0 e
//! "estouram" (freada, motor, explosão). Aqui o ganho baixa na hora em que um
//! pico passaria do teto e volta devagar; abaixo do teto nada muda.

use super::contracts::FrameSink;
use super::packet::{VideoPacket, HEADER};
use axum::body::Bytes;
use std::sync::{Arc, Mutex};

/// O teto, um pouco abaixo do 1,0 para sobrar margem ao Opus.
const CEILING: f32 = 0.95;
/// Quanto tempo o ganho leva para voltar (a constante de tempo).
const RELEASE_SECONDS: f32 = 0.2;
const SAMPLE_RATE: f32 = 48_000.0;

/// O estado do limitador: o ganho de agora (1,0 = sem mexer).
pub struct Limiter {
    gain: f32,
    release: f32,
    /// O maior pico visto (antes do limitador), para o log.
    pub peak: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self { gain: 1.0, release: 1.0 - (-1.0 / (RELEASE_SECONDS * SAMPLE_RATE)).exp(), peak: 0.0 }
    }
}

impl Limiter {
    /// Limita amostras estéreo intercaladas (L, R, L, R...). Os dois canais
    /// usam o mesmo ganho, para o som não mudar de lado.
    pub fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.chunks_exact_mut(2) {
            let level = frame[0].abs().max(frame[1].abs());
            if level > self.peak && level.is_finite() {
                self.peak = level;
            }
            let needed = if level > CEILING { CEILING / level } else { 1.0 };
            if needed < self.gain {
                self.gain = needed;
            } else {
                self.gain += (1.0 - self.gain) * self.release;
                self.gain = self.gain.min(needed);
            }
            if self.gain < 1.0 {
                for s in frame.iter_mut() {
                    *s *= self.gain;
                }
            }
            // um valor inválido (NaN, infinito) vindo do sistema vira silêncio
            for s in frame.iter_mut() {
                if !s.is_finite() {
                    *s = 0.0;
                }
            }
        }
    }
}

/// Um destino que passa o som pelo limitador antes de entregar.
pub struct Limited {
    inner: Arc<dyn FrameSink>,
    limiter: Mutex<Limiter>,
    /// Já avisou no log que o som passou do teto.
    warned: Mutex<bool>,
}

impl Limited {
    pub fn new(inner: Arc<dyn FrameSink>) -> Self {
        Self { inner, limiter: Mutex::new(Limiter::default()), warned: Mutex::new(false) }
    }
}

impl FrameSink for Limited {
    fn video(&self, packet: VideoPacket) {
        self.inner.video(packet);
    }

    fn audio(&self, packet: Bytes) {
        if packet.len() <= HEADER {
            return self.inner.audio(packet);
        }
        let mut samples: Vec<f32> = packet[HEADER..]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let peak = {
            let mut limiter = self.limiter.lock().unwrap();
            limiter.process(&mut samples);
            limiter.peak
        };
        if peak > 1.0 {
            let mut warned = self.warned.lock().unwrap();
            if !*warned {
                *warned = true;
                crate::log_info!(
                    "audio",
                    "The shared sound went above full volume (peak {peak:.2}), so it is being turned down at the loud moments to avoid distortion for viewers"
                );
            }
        }
        let mut out = Vec::with_capacity(packet.len());
        out.extend_from_slice(&packet[..HEADER]);
        out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        self.inner.audio(Bytes::from(out));
    }

    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` de um tom de 100 Hz estéreo com a amplitude `amp`.
    fn tone(amp: f32, seconds: f32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE) as usize;
        (0..n)
            .flat_map(|i| {
                let s = (i as f32 * 100.0 * std::f32::consts::TAU / SAMPLE_RATE).sin() * amp;
                [s, -s]
            })
            .collect()
    }

    #[test]
    fn quiet_sound_is_untouched() {
        let original = tone(0.9, 0.5);
        let mut samples = original.clone();
        Limiter::default().process(&mut samples);
        assert_eq!(samples, original);
    }

    #[test]
    fn loud_sound_never_goes_above_the_ceiling() {
        let mut samples = tone(1.8, 0.5);
        let mut limiter = Limiter::default();
        limiter.process(&mut samples);
        let max = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(max <= CEILING + 1e-6, "peak after the limiter: {max}");
        assert!((limiter.peak - 1.8).abs() < 0.01);
    }

    #[test]
    fn volume_comes_back_after_the_loud_moment() {
        let mut limiter = Limiter::default();
        limiter.process(&mut tone(2.0, 0.2));
        assert!(limiter.gain < 0.5);
        limiter.process(&mut tone(0.3, 1.0));
        assert!(limiter.gain > 0.99, "gain 1 s later: {}", limiter.gain);
    }

    #[test]
    fn invalid_samples_become_silence() {
        let mut samples = vec![f32::NAN, 0.5, f32::INFINITY, -0.2];
        Limiter::default().process(&mut samples);
        assert!(samples.iter().all(|s| s.is_finite()));
    }
}
