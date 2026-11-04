//! Formato dos pacotes que a captura entrega ao app (lido em
//! `src/lib/capture/packet.ts`). Os dois lados conferem os mesmos exemplos em
//! `src/lib/capture/packet-fixture.json`, então mudar o formato quebra o teste
//! dos dois.
//!
//! Cabeçalho de 16 bytes: `[tipo, formato, largura u16 LE, altura u16 LE, 0...]`,
//! seguido dos dados: vídeo em I420 ou NV12 (1,5 byte por pixel, BT.709 de
//! faixa limitada) ou áudio em f32 estéreo intercalado a 48 kHz.

use axum::body::Bytes;

pub const KIND_VIDEO: u8 = 1;
pub const KIND_AUDIO: u8 = 2;
pub const HEADER: usize = 16;

/// Como os pixels do vídeo estão organizados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFormat {
    /// Planos Y, U e V separados (convertido no processador).
    I420 = 0,
    /// Plano Y e plano UV intercalado (convertido na placa de vídeo).
    Nv12 = 1,
}

impl PixelFormat {
    /// Onde a conversão aconteceu, para os logs.
    pub fn converted_on(self) -> &'static str {
        match self {
            PixelFormat::I420 => "CPU",
            PixelFormat::Nv12 => "GPU",
        }
    }
}

/// Um frame de vídeo pronto para enviar: cabeçalho + pixels.
pub struct VideoPacket(Vec<u8>);

impl VideoPacket {
    /// Pacote com o cabeçalho preenchido e os pixels zerados (`w` e `h` pares).
    pub fn new(format: PixelFormat, w: u32, h: u32) -> Self {
        let mut pkt = vec![0u8; HEADER + (w * h * 3 / 2) as usize];
        pkt[0] = KIND_VIDEO;
        pkt[1] = format as u8;
        pkt[2..4].copy_from_slice(&(w as u16).to_le_bytes());
        pkt[4..6].copy_from_slice(&(h as u16).to_le_bytes());
        Self(pkt)
    }

    pub fn format(&self) -> PixelFormat {
        if self.0[1] == PixelFormat::Nv12 as u8 {
            PixelFormat::Nv12
        } else {
            PixelFormat::I420
        }
    }

    pub fn width(&self) -> u16 {
        u16::from_le_bytes([self.0[2], self.0[3]])
    }

    pub fn height(&self) -> u16 {
        u16::from_le_bytes([self.0[4], self.0[5]])
    }

    /// Onde escrever os pixels.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.0[HEADER..]
    }

    pub fn into_bytes(self) -> Bytes {
        Bytes::from(self.0)
    }
}

/// Pacote de áudio com as amostras (f32 estéreo, em bytes).
pub fn audio_packet(samples: impl IntoIterator<Item = u8>) -> Bytes {
    let mut pkt = vec![0u8; HEADER];
    pkt[0] = KIND_AUDIO;
    pkt.extend(samples);
    Bytes::from(pkt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const FIXTURE: &str = include_str!("../../../src/lib/capture/packet-fixture.json");

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn constants_match_the_fixture() {
        let f: Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(f["header"], HEADER);
        assert_eq!(f["kind"]["video"], KIND_VIDEO);
        assert_eq!(f["kind"]["audio"], KIND_AUDIO);
        assert_eq!(f["format"]["i420"], PixelFormat::I420 as u8);
        assert_eq!(f["format"]["nv12"], PixelFormat::Nv12 as u8);
    }

    #[test]
    fn packets_match_the_fixture() {
        let f: Value = serde_json::from_str(FIXTURE).unwrap();
        for ex in f["examples"].as_array().unwrap() {
            let name = ex["name"].as_str().unwrap();
            let bytes = match ex["kind"].as_u64().unwrap() as u8 {
                KIND_VIDEO => {
                    let format = if ex["format"] == 1 { PixelFormat::Nv12 } else { PixelFormat::I420 };
                    let w = ex["width"].as_u64().unwrap() as u32;
                    let h = ex["height"].as_u64().unwrap() as u32;
                    let pkt = VideoPacket::new(format, w, h);
                    assert_eq!(pkt.format(), format, "{name}");
                    assert_eq!((pkt.width() as u32, pkt.height() as u32), (w, h), "{name}");
                    pkt.into_bytes()
                }
                _ => audio_packet(vec![0u8; ex["payload"].as_u64().unwrap() as usize]),
            };
            assert_eq!(hex(&bytes[..HEADER]), ex["header_hex"].as_str().unwrap(), "{name}");
            assert_eq!(bytes.len() - HEADER, ex["payload"].as_u64().unwrap() as usize, "{name}");
        }
    }
}
