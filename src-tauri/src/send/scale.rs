//! Reduz um frame da captura (NV12 ou I420) para a altura de uma camada de
//! qualidade (`layers.rs`), mantendo a proporção. No processador, plano por
//! plano (Y inteiro, cor pela metade), em ~1–3 ms para 1080p → 720p.

use crate::capture::{
    contracts::BoxError,
    packet::{PixelFormat, VideoPacket},
};
use fast_image_resize as fr;

#[derive(Default)]
pub struct Scaler {
    resizer: fr::Resizer,
}

/// Largura e altura (pares, como o NV12 exige) para `height` mantendo a proporção.
pub fn scaled_size(w: u32, h: u32, height: u32) -> (u32, u32) {
    let even = |v: u32| (v.max(2) / 2) * 2;
    let height = even(height.min(h));
    (even((w as u64 * height as u64 / h.max(1) as u64) as u32), height)
}

impl Scaler {
    /// O frame com `height` de altura (o mesmo frame se já for desse tamanho).
    pub fn scale(&mut self, frame: &VideoPacket, height: u32) -> Result<VideoPacket, BoxError> {
        let (sw, sh) = (frame.width() as u32, frame.height() as u32);
        let (dw, dh) = scaled_size(sw, sh, height);
        if (dw, dh) == (sw, sh) {
            return Ok(frame.clone());
        }
        let format = frame.format();
        let mut out = VideoPacket::new(format, dw, dh);
        let src = frame.pixels();
        let dst = out.pixels_mut();
        let (sy, dy) = ((sw * sh) as usize, (dw * dh) as usize);
        // sem alfa: o U8x2 da cor intercalada (U, V) não é "cinza + transparência"
        let opts = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear))
            .use_alpha(false);
        let mut plane = |s: &[u8], d: &mut [u8], (w, h, dw, dh): (u32, u32, u32, u32), px: fr::PixelType| {
            let s = fr::images::ImageRef::new(w, h, s, px)?;
            let mut d = fr::images::Image::from_slice_u8(dw, dh, d, px)?;
            self.resizer.resize(&s, &mut d, &opts)?;
            Ok::<_, BoxError>(())
        };
        let half = (sw / 2, sh / 2, dw / 2, dh / 2);
        plane(&src[..sy], &mut dst[..dy], (sw, sh, dw, dh), fr::PixelType::U8)?;
        match format {
            // cor intercalada (U, V, U, V...): um plano de pares
            PixelFormat::Nv12 => plane(&src[sy..sy + sy / 2], &mut dst[dy..dy + dy / 2], half, fr::PixelType::U8x2)?,
            PixelFormat::I420 => {
                let (sq, dq) = (sy / 4, dy / 4);
                plane(&src[sy..sy + sq], &mut dst[dy..dy + dq], half, fr::PixelType::U8)?;
                plane(&src[sy + sq..sy + 2 * sq], &mut dst[dy + dq..dy + 2 * dq], half, fr::PixelType::U8)?;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_aspect_and_even_sizes() {
        assert_eq!(scaled_size(1920, 1080, 720), (1280, 720));
        assert_eq!(scaled_size(1920, 1032, 720), (1338, 720));
        assert_eq!(scaled_size(1338, 720, 1080), (1338, 720));
        assert_eq!(scaled_size(2560, 1440, 361), (640, 360));
    }

    #[test]
    fn scales_nv12_and_i420_keeping_colors() {
        for format in [PixelFormat::Nv12, PixelFormat::I420] {
            let mut pkt = VideoPacket::new(format, 64, 36);
            let (y, uv) = pkt.pixels_mut().split_at_mut(64 * 36);
            y.fill(200);
            uv.fill(90);
            let out = Scaler::default().scale(&pkt, 18).unwrap();
            assert_eq!((out.width(), out.height(), out.format()), (32, 18, format));
            let (y, uv) = out.pixels().split_at(32 * 18);
            assert!(y.iter().all(|&v| v == 200), "{format:?} luma {:?}", &y[..8]);
            assert_eq!(uv.len(), 32 * 18 / 2);
            assert!(uv.iter().all(|&v| v == 90), "{format:?} color");
        }
    }
}
