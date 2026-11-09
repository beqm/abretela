//! Áudio do envio nativo (ROADMAP, seção 2, parte 3c): o som da captura
//! (f32 estéreo a 48 kHz, em pedaços de 10 ms) vira Opus em pacotes de 20 ms,
//! codificado uma vez e mandado a todos, como o vídeo.
//!
//! Opus pela `opus-pure` (Rust puro, sem C nem cmake; o resultado decodifica
//! limpo na libopus de referência). Cada pacote leva o tempo desde o começo
//! da transmissão, o mesmo relógio do vídeo, para o navegador sincronizar os
//! dois. Quando o app fica em silêncio o Windows pode parar de mandar
//! pedaços; na volta o tempo pula para "agora" em vez de ficar atrasado.

use super::Encoded;
use opus_pure::{Application, OpusEncoder, MAX_PACKET_BYTES};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// 20 ms por pacote (o que os navegadores usam).
const FRAME: usize = SAMPLE_RATE as usize / 50;
const FRAME_TIME: Duration = Duration::from_millis(20);
/// Som de jogo e música, não só voz: bem acima dos ~32 kbps de uma chamada.
const BITRATE: i32 = 128_000;
/// Se o som chegar mais de tanto fora do esperado, recomeça o tempo.
const RESYNC_AFTER: Duration = Duration::from_millis(200);

pub struct AudioEncoder {
    opus: OpusEncoder,
    started: Instant,
    /// Amostras intercaladas (L, R, L, R...) esperando completar 20 ms.
    pending: Vec<f32>,
    /// Tempo (desde o começo) da primeira amostra em `pending`.
    next_at: Option<Duration>,
    out: Vec<u8>,
    pub packets: u64,
    pub resyncs: u64,
}

impl AudioEncoder {
    pub fn new(started: Instant) -> Result<Self, String> {
        let mut opus = OpusEncoder::new(SAMPLE_RATE as i32, CHANNELS, Application::Audio).map_err(|e| e.to_string())?;
        opus.bitrate_bps = BITRATE;
        // recupera um pacote perdido a partir do seguinte (o navegador pede isso)
        opus.use_inband_fec = true;
        opus.packet_loss_perc = 5;
        Ok(Self {
            opus,
            started,
            pending: Vec::with_capacity(FRAME * CHANNELS * 2),
            next_at: None,
            out: vec![0; MAX_PACKET_BYTES],
            packets: 0,
            resyncs: 0,
        })
    }

    /// Ninguém ouvindo: joga fora o que estava esperando.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.next_at = None;
    }

    /// Um pedaço da captura (f32 estéreo em bytes, little-endian) que chegou
    /// em `now`; devolve os pacotes Opus que ficaram prontos.
    pub fn push(&mut self, bytes: &[u8], now: Instant) -> Result<Vec<Encoded>, String> {
        let samples = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let frames = bytes.len() / 4 / CHANNELS;
        let chunk_start = now.saturating_duration_since(self.started).saturating_sub(duration_of(frames));
        let expected = self.next_at.map(|t| t + duration_of(self.pending.len() / CHANNELS));
        if expected.is_none_or(|e| e.abs_diff(chunk_start) > RESYNC_AFTER) {
            if expected.is_some() {
                self.resyncs += 1;
            }
            self.pending.clear();
            self.next_at = Some(chunk_start);
        }
        self.pending.extend(samples);

        let mut ready = Vec::new();
        while self.pending.len() >= FRAME * CHANNELS {
            let n = self
                .opus
                .encode(&self.pending[..FRAME * CHANNELS], FRAME, &mut self.out)
                .map_err(|e| format!("Opus failed: {e}"))?;
            self.pending.drain(..FRAME * CHANNELS);
            let at = self.next_at.unwrap_or_default();
            self.next_at = Some(at + FRAME_TIME);
            self.packets += 1;
            ready.push(Encoded { data: self.out[..n].into(), keyframe: false, timestamp: at });
        }
        Ok(ready)
    }
}

fn duration_of(frames: usize) -> Duration {
    Duration::from_micros(frames as u64 * 1_000_000 / SAMPLE_RATE as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 ms de um tom, como a captura manda.
    fn chunk(n: usize) -> Vec<u8> {
        (0..480)
            .flat_map(|i| {
                let s = ((n * 480 + i) as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                [s, s]
            })
            .flat_map(f32::to_le_bytes)
            .collect()
    }

    #[test]
    fn two_chunks_make_one_packet_with_steady_time() {
        let start = Instant::now();
        let mut enc = AudioEncoder::new(start).unwrap();
        let mut packets = Vec::new();
        for n in 0..10 {
            let now = start + Duration::from_millis(10 * (n as u64 + 1));
            packets.extend(enc.push(&chunk(n), now).unwrap());
        }
        assert_eq!(packets.len(), 5);
        let times: Vec<_> = packets.iter().map(|p| p.timestamp.as_millis()).collect();
        assert_eq!(times, [0, 20, 40, 60, 80]);
        assert!(packets.iter().all(|p| !p.data.is_empty() && p.data.len() < 1000));
        assert_eq!(enc.resyncs, 0);
    }

    #[test]
    fn silence_gap_jumps_to_now() {
        let start = Instant::now();
        let mut enc = AudioEncoder::new(start).unwrap();
        for n in 0..4 {
            enc.push(&chunk(n), start + Duration::from_millis(10 * (n as u64 + 1))).unwrap();
        }
        // 2 s sem nada (o app ficou quieto) e o som volta
        let back = start + Duration::from_millis(2_050);
        let mut packets = enc.push(&chunk(4), back).unwrap();
        packets.extend(enc.push(&chunk(5), back + Duration::from_millis(10)).unwrap());
        assert_eq!(enc.resyncs, 1);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].timestamp.as_millis(), 2_040);
    }
}
