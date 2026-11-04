//! Conversão no processador, igual em qualquer sistema: reduzir uma imagem
//! BGRA e passar para I420 (BT.709, faixa limitada).

use super::contracts::BoxError;
use fast_image_resize as fr;
use rayon::prelude::*;
use std::{sync::OnceLock, thread};

/// Tamanho final que cabe em `max_w` x `max_h` mantendo a proporção, sem
/// ampliar e com dimensões pares (exigência do I420/NV12).
pub fn target_size(sw: u32, sh: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f64 / sw as f64).min(max_h as f64 / sh as f64).min(1.0);
    let dw = (((sw as f64 * scale) as u32) & !1).max(2);
    let dh = (((sh as f64 * scale) as u32) & !1).max(2);
    (dw, dh)
}

/// Reduz imagens BGRA (reaproveita o redimensionador entre frames).
#[derive(Default)]
pub struct BgraScaler {
    resizer: fr::Resizer,
}

impl BgraScaler {
    /// Reduz `src` (`sw` x `sh`, BGRA sem padding) para caber em `max_w` x
    /// `max_h`. Retorna (largura, altura, bgra).
    pub fn scale(&mut self, src: &[u8], sw: u32, sh: u32, max_w: u32, max_h: u32) -> Result<(u32, u32, Vec<u8>), BoxError> {
        let (dw, dh) = target_size(sw, sh, max_w, max_h);
        if (dw, dh) == (sw, sh) {
            return Ok((dw, dh, src[..(dw * dh * 4) as usize].to_vec()));
        }
        let src_img = fr::images::ImageRef::new(sw, sh, src, fr::PixelType::U8x4)?;
        let mut dst = fr::images::Image::new(dw, dh, fr::PixelType::U8x4);
        let opts = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear))
            .use_alpha(false);
        self.resizer.resize(&src_img, &mut dst, &opts)?;
        Ok((dw, dh, dst.into_vec()))
    }
}

/// Poucas threads e de baixa prioridade para a conversão no processador: o
/// pool global do rayon usaria todos os núcleos a cada frame.
fn convert_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 4).clamp(2, 4))
            .thread_name(|i| format!("capture-convert-{i}"))
            .start_handler(|_| super::platform::lower_thread_priority())
            .build()
            .expect("pool de conversão")
    })
}

/// BGRA -> I420 (BT.709, faixa limitada), em paralelo por pares de linhas.
pub fn bgra_to_i420(w: usize, h: usize, bgra: &[u8], out: &mut [u8]) {
    convert_pool().install(|| bgra_to_i420_rows(w, h, bgra, out));
}

fn bgra_to_i420_rows(w: usize, h: usize, bgra: &[u8], out: &mut [u8]) {
    let (y_plane, uv) = out.split_at_mut(w * h);
    let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
    let cw = w / 2;

    y_plane
        .par_chunks_mut(w * 2)
        .zip(u_plane.par_chunks_mut(cw))
        .zip(v_plane.par_chunks_mut(cw))
        .enumerate()
        .for_each(|(row, ((y2, u), v))| {
            let src0 = &bgra[row * 2 * w * 4..][..w * 4];
            let src1 = &bgra[(row * 2 + 1) * w * 4..][..w * 4];
            let (y0, y1) = y2.split_at_mut(w);
            for x in 0..cw {
                let mut rs = 0i32;
                let mut gs = 0i32;
                let mut bs = 0i32;
                for (src, ys) in [(src0, &mut *y0), (src1, &mut *y1)] {
                    for dx in 0..2 {
                        let px = x * 2 + dx;
                        let b = src[px * 4] as i32;
                        let g = src[px * 4 + 1] as i32;
                        let r = src[px * 4 + 2] as i32;
                        ys[px] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
                        rs += r;
                        gs += g;
                        bs += b;
                    }
                }
                let (r, g, b) = (rs / 4, gs / 4, bs / 4);
                u[x] = (((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
                v[x] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_keeps_ratio_never_upscales_and_is_even() {
        assert_eq!(target_size(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(target_size(1280, 720, 1920, 1080), (1280, 720));
        // ultrawide limitado pela largura
        assert_eq!(target_size(3440, 1440, 1680, 720), (1680, 702));
        // ímpar vira par
        assert_eq!(target_size(801, 601, 4000, 4000), (800, 600));
        assert_eq!(target_size(1, 1, 10, 10), (2, 2));
    }

    /// Converte uma imagem de uma cor só e confere o Y, U e V esperados.
    fn i420_of(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
        let (w, h) = (4, 2);
        let bgra: Vec<u8> = (0..w * h).flat_map(|_| [b, g, r, 255]).collect();
        let mut out = vec![0u8; w * h * 3 / 2];
        bgra_to_i420(w, h, &bgra, &mut out);
        assert!(out[..w * h].iter().all(|&y| y == out[0]));
        (out[0], out[w * h], out[w * h + w * h / 4])
    }

    #[test]
    fn bt709_limited_range_colors() {
        // cinza não tem cor: U e V no meio (o arredondamento pode dar 127)
        let neutral = |(y, u, v): (u8, u8, u8)| (y, u.abs_diff(128) <= 1 && v.abs_diff(128) <= 1);
        assert_eq!(neutral(i420_of(0, 0, 0)), (16, true));
        assert_eq!(neutral(i420_of(255, 255, 255)), (235, true));
        let (y, u, v) = i420_of(0, 0, 255); // vermelho
        assert_eq!(y, 63);
        assert!(u < 128 && v > 200, "u={u} v={v}");
        let (_, u, v) = i420_of(255, 0, 0); // azul
        assert!(u > 200 && v < 128, "u={u} v={v}");
    }

    #[test]
    fn scaler_reduces_and_copies() {
        let mut s = BgraScaler::default();
        let src = vec![200u8; 8 * 4 * 4];
        let (w, h, out) = s.scale(&src, 8, 4, 4, 4).unwrap();
        assert_eq!((w, h, out.len()), (4, 2, 4 * 2 * 4));
        let (w, h, out) = s.scale(&src, 8, 4, 100, 100).unwrap();
        assert_eq!((w, h), (8, 4));
        assert_eq!(out, src);
    }
}
