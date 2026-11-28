//! O que a captura do Linux faz com cada frame antes de converter, sem
//! depender do PipeWire (para poder testar): tirar o enchimento das linhas,
//! pôr as cores na ordem que o resto do app espera (BGRA) e segurar o ritmo.

use std::time::{Duration, Instant};

/// A ordem das cores em cada pixel de 4 bytes que o PipeWire entrega.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Azul, verde, vermelho e um byte que não importa (BGRx, BGRA).
    Bgrx,
    /// Vermelho, verde, azul e um byte que não importa (RGBx, RGBA).
    Rgbx,
}

/// Copia um frame para `out` como BGRA sem enchimento (`w * h * 4` bytes).
///
/// `src` tem `stride` bytes por linha, que pode ser mais que `w * 4`: o
/// sistema alinha as linhas. Devolve `false`, sem mexer em `out`, se `src`
/// for menor do que o frame precisa.
pub fn pack_bgra(src: &[u8], stride: usize, w: usize, h: usize, layout: Layout, out: &mut Vec<u8>) -> bool {
    let row = w * 4;
    if w == 0 || h == 0 || stride < row || src.len() < stride * (h - 1) + row {
        return false;
    }
    out.clear();
    out.reserve(row * h);
    for line in src.chunks(stride).take(h) {
        out.extend_from_slice(&line[..row]);
    }
    if layout == Layout::Rgbx {
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    true
}

/// Um pedaço de um frame, em pixels: de onde começa e o tamanho.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// O pedaço de um frame de `w` por `h` que tem imagem.
///
/// Ao transmitir uma janela, o GNOME entrega frames do tamanho da tela e diz
/// à parte (`crop`: x, y, largura e altura) onde a janela está neles; o resto
/// vem vazio. Sem esse aviso vale o frame inteiro. Um aviso que passa da
/// borda é cortado nela, e um que não sobra nada é ignorado.
pub fn visible_part(w: usize, h: usize, crop: Option<(i32, i32, u32, u32)>) -> Part {
    let whole = Part { x: 0, y: 0, w, h };
    let Some((x, y, cw, ch)) = crop else { return whole };
    // o que fica à esquerda ou acima do frame não existe
    let (left, top) = (x.max(0) as usize, y.max(0) as usize);
    let right = (x as i64 + cw as i64).clamp(0, w as i64) as usize;
    let bottom = (y as i64 + ch as i64).clamp(0, h as i64) as usize;
    if left >= right || top >= bottom {
        return whole;
    }
    Part { x: left, y: top, w: right - left, h: bottom - top }
}

/// Como [`pack_bgra`], mas copia só o pedaço `part` do frame.
pub fn pack_part(src: &[u8], stride: usize, part: Part, layout: Layout, out: &mut Vec<u8>) -> bool {
    match src.get(part.y * stride + part.x * 4..) {
        Some(from) => pack_bgra(from, stride, part.w, part.h, layout, out),
        None => false,
    }
}

/// Segura o ritmo: o sistema entrega frames na frequência do monitor, e o app
/// só quer os da taxa escolhida.
pub struct Pacer {
    interval: Duration,
    next: Option<Instant>,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        Self { interval: Duration::from_secs(1) / fps.max(1), next: None }
    }

    /// Este frame, que chegou em `now`, entra?
    pub fn due(&mut self, now: Instant) -> bool {
        // um frame que chega um pouco antes da hora ainda entra: os do sistema
        // não vêm em intervalos exatos, e esperar o seguinte cortaria a taxa
        // pela metade (60 fps do monitor viram 30, não 20)
        let slack = self.interval / 4;
        match self.next {
            Some(next) if now + slack < next => false,
            Some(next) => {
                // segue a grade de tempo, a não ser que tenha ficado muito para trás
                self.next = Some(if now > next + self.interval { now + self.interval } else { next + self.interval });
                true
            }
            None => {
                self.next = Some(now + self.interval);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_the_padding_at_the_end_of_each_line() {
        // 2x2, com 4 bytes de enchimento por linha
        let src = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 0, 0, 0, 0];
        let mut out = vec![99];
        assert!(pack_bgra(&src, 12, 2, 2, Layout::Bgrx, &mut out));
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    }

    #[test]
    fn the_last_line_does_not_need_its_padding() {
        let src = [1, 2, 3, 4, 0, 0, 5, 6, 7, 8];
        let mut out = Vec::new();
        assert!(pack_bgra(&src, 6, 1, 2, Layout::Bgrx, &mut out));
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn rgb_frames_come_out_as_bgr() {
        // um pixel vermelho e um azul, em RGBx
        let src = [255, 0, 0, 255, 0, 0, 255, 255];
        let mut out = Vec::new();
        assert!(pack_bgra(&src, 8, 2, 1, Layout::Rgbx, &mut out));
        assert_eq!(out, [0, 0, 255, 255, 255, 0, 0, 255], "blue first: red is now the third byte");
    }

    #[test]
    fn a_frame_smaller_than_its_size_says_is_refused() {
        let mut out = vec![7];
        assert!(!pack_bgra(&[0; 15], 8, 2, 2, Layout::Bgrx, &mut out), "the second line is one byte short");
        assert!(!pack_bgra(&[0; 16], 4, 2, 2, Layout::Bgrx, &mut out), "a line cannot be shorter than its pixels");
        assert!(!pack_bgra(&[], 0, 0, 0, Layout::Bgrx, &mut out));
        assert_eq!(out, [7], "what was there stays");
    }

    #[test]
    fn without_a_crop_the_whole_frame_counts() {
        assert_eq!(visible_part(1280, 800, None), Part { x: 0, y: 0, w: 1280, h: 800 });
    }

    #[test]
    fn a_window_inside_a_screen_sized_frame_is_cut_out() {
        // a janela dos arquivos na VM: 890x550 num frame de 1280x800
        assert_eq!(visible_part(1280, 800, Some((116, 78, 890, 550))), Part { x: 116, y: 78, w: 890, h: 550 });
    }

    #[test]
    fn a_crop_past_the_edges_is_cut_at_them() {
        // metade para fora, à direita e embaixo
        assert_eq!(visible_part(1280, 800, Some((1000, 600, 890, 550))), Part { x: 1000, y: 600, w: 280, h: 200 });
        // e para fora à esquerda e em cima
        assert_eq!(visible_part(1280, 800, Some((-100, -50, 890, 550))), Part { x: 0, y: 0, w: 790, h: 500 });
    }

    #[test]
    fn a_crop_with_nothing_left_is_ignored() {
        let whole = Part { x: 0, y: 0, w: 1280, h: 800 };
        assert_eq!(visible_part(1280, 800, Some((0, 0, 0, 0))), whole);
        assert_eq!(visible_part(1280, 800, Some((2000, 10, 100, 100))), whole, "all of it is outside");
        assert_eq!(visible_part(1280, 800, Some((-500, 10, 100, 100))), whole);
        assert_eq!(visible_part(1280, 800, Some((i32::MAX, i32::MAX, u32::MAX, u32::MAX))), whole, "no overflow");
    }

    #[test]
    fn copies_only_the_part_asked_for() {
        // 3x3 com 2 bytes de enchimento por linha; o pixel de cada posição vale linha*10 + coluna
        let mut src = Vec::new();
        for line in 0..3u8 {
            for col in 0..3u8 {
                src.extend_from_slice(&[line * 10 + col; 4]);
            }
            src.extend_from_slice(&[0, 0]);
        }
        let mut out = Vec::new();
        // o canto de baixo à direita, 2x2
        assert!(pack_part(&src, 14, Part { x: 1, y: 1, w: 2, h: 2 }, Layout::Bgrx, &mut out));
        assert_eq!(out, [11, 11, 11, 11, 12, 12, 12, 12, 21, 21, 21, 21, 22, 22, 22, 22]);
        assert!(!pack_part(&src, 14, Part { x: 1, y: 1, w: 2, h: 3 }, Layout::Bgrx, &mut out), "one line too many");
        assert!(!pack_part(&src, 14, Part { x: 0, y: 9, w: 1, h: 1 }, Layout::Bgrx, &mut out), "starts past the end");
    }

    /// Quantos frames entram em um segundo, vindos de um monitor de `hz` Hz.
    fn accepted(fps: u32, hz: u32) -> u32 {
        let mut pacer = Pacer::new(fps);
        let start = Instant::now();
        (0..hz).filter(|&n| pacer.due(start + Duration::from_secs(1) * n / hz)).count() as u32
    }

    #[test]
    fn keeps_the_chosen_rate_whatever_the_monitor_sends() {
        assert_eq!(accepted(30, 60), 30, "a 60 Hz monitor at 30 fps: every other frame");
        assert_eq!(accepted(60, 60), 60, "the same rate: every frame");
        assert!((29..=31).contains(&accepted(30, 144)), "144 Hz at 30 fps: {}", accepted(30, 144));
        assert!((59..=61).contains(&accepted(60, 144)), "144 Hz at 60 fps: {}", accepted(60, 144));
        assert_eq!(accepted(60, 30), 30, "a slower source is not held back");
    }

    #[test]
    fn comes_back_at_once_after_a_pause() {
        let mut pacer = Pacer::new(30);
        let start = Instant::now();
        assert!(pacer.due(start));
        // a tela ficou parada por 5 s (o sistema só manda frames quando algo muda)
        let later = start + Duration::from_secs(5);
        assert!(pacer.due(later), "the first frame after the pause goes through");
        assert!(!pacer.due(later + Duration::from_millis(5)), "and the rate holds from there");
        assert!(pacer.due(later + Duration::from_millis(34)));
    }
}
