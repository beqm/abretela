//! Camadas de qualidade do envio nativo (ROADMAP, seção 2, parte 3d).
//!
//! Em vez de uma codificação só, com o bitrate puxado pelo espectador mais
//! lento, há uma codificação por **camada** (altura): a da transmissão e as
//! menores (720p, 480p, 360p). Cada espectador recebe uma camada:
//! - a que pediu (a mensagem "view": o tamanho em que está vendo; 0 = app
//!   escondido, só som), como no envio pelo WebView (`encodingFor` em
//!   `src/lib/quality.ts`, com a mesma tabela de bitrate);
//! - ou uma menor, se a conexão dele não aguenta: desce quando a banda
//!   estimada fica abaixo da metade do que a camada pede por 3 s seguidos
//!   (a estimativa oscila), e sobe de volta só depois de 10 s com folga, para
//!   não ficar trocando.
//!
//! Só são codificadas as camadas que alguém está recebendo; dentro de uma
//! camada o bitrate segue o mais lento dela, mas nunca abaixo do ponto em que
//! ele desceria de camada, então um espectador lento não derruba os outros.

use std::time::{Duration, Instant};

/// Alturas das camadas menores (as mesmas opções de qualidade do app).
pub const TIERS: [u32; 4] = [360, 480, 720, 1080];
/// "Camada" de quem está com o app escondido: sem vídeo, só som.
pub const NO_VIDEO: u32 = 0;
/// Desce de camada com a banda abaixo dessa fração do que a camada pede.
const DOWN_BELOW: f64 = 0.5;
/// ...por esse tempo seguido.
pub const DOWN_AFTER: Duration = Duration::from_secs(3);
/// Sobe com a banda acima dessa fração do que a camada de cima pede...
const UP_ABOVE: f64 = 0.8;
/// ...por esse tempo seguido.
pub const UP_AFTER: Duration = Duration::from_secs(10);
/// Menor bitrate que uma camada pode usar.
pub const MIN_BITRATE: u32 = 150_000;

/// O mesmo `videoBitrate` de `src/lib/quality.ts`.
pub fn tier_bitrate(height: u32, fps: u32) -> u32 {
    let base: f64 = match height {
        0..=360 => 800_000.0,
        361..=480 => 1_200_000.0,
        481..=720 => 2_500_000.0,
        _ => 4_500_000.0,
    };
    let factor = if fps >= 60 {
        1.6
    } else if fps <= 15 {
        0.7
    } else {
        1.0
    };
    (base * factor).round() as u32
}

/// Desde quando um espectador está sem banda para a camada atual, e desde
/// quando tem folga para a de cima.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timers {
    pub worse_since: Option<Instant>,
    pub better_since: Option<Instant>,
}

/// A transmissão: altura dos frames, fps e bitrate máximo escolhidos.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub top: u32,
    pub fps: u32,
    pub max_bitrate: u32,
}

impl Plan {
    /// As camadas possíveis, da menor para a maior (a última é a transmissão).
    pub fn layers(&self) -> Vec<u32> {
        let mut v: Vec<u32> = TIERS.iter().copied().filter(|&t| (t as f64) < self.top as f64 * 0.95).collect();
        v.push(self.top);
        v
    }

    /// fps de uma camada: miniaturas (até 480p, abaixo da transmissão) vão a
    /// no máximo 30, como no envio pelo WebView.
    pub fn fps(&self, layer: u32) -> u32 {
        if layer < self.top && layer <= 480 {
            self.fps.min(30)
        } else {
            self.fps
        }
    }

    /// O bitrate máximo de uma camada.
    pub fn rate(&self, layer: u32) -> u32 {
        if layer >= self.top {
            self.max_bitrate
        } else {
            tier_bitrate(layer, self.fps(layer)).min(self.max_bitrate)
        }
    }

    /// A camada que alguém pediu: a menor que cobre a altura pedida (sem
    /// pedido, a transmissão inteira; 0 = sem vídeo).
    pub fn wanted(&self, wants: Option<u32>) -> u32 {
        match wants {
            None => self.top,
            Some(NO_VIDEO) => NO_VIDEO,
            Some(h) => self.layers().into_iter().find(|&l| l >= h).unwrap_or(self.top),
        }
    }

    /// Para onde um espectador vai agora. `estimate` é a banda até ele (sem o
    /// áudio); `timers` guarda desde quando ele está sem banda para a camada
    /// atual e desde quando tem folga para a de cima.
    pub fn choose(
        &self,
        wants: Option<u32>,
        estimate: Option<u64>,
        current: Option<u32>,
        timers: &mut Timers,
        now: Instant,
    ) -> u32 {
        let target = self.wanted(wants);
        if target == NO_VIDEO {
            *timers = Timers::default();
            return NO_VIDEO;
        }
        let layers: Vec<u32> = self.layers().into_iter().filter(|&l| l <= target).collect();
        let fits = |l: u32, frac: f64| estimate.is_none_or(|e| e as f64 >= self.rate(l) as f64 * frac);
        let index = |l: u32| layers.iter().position(|&x| x == l);
        let Some(mut i) = current.and_then(index).filter(|_| current != Some(NO_VIDEO)) else {
            // começando (ou pediu menos do que recebia): a pedida, ou a maior
            // abaixo dela que a conexão aguenta
            *timers = Timers::default();
            let i = (0..layers.len()).rev().find(|&i| fits(layers[i], DOWN_BELOW)).unwrap_or(0);
            return layers[i];
        };
        if i > 0 && !fits(layers[i], DOWN_BELOW) {
            timers.better_since = None;
            let since = *timers.worse_since.get_or_insert(now);
            if now.duration_since(since) >= DOWN_AFTER {
                timers.worse_since = None;
                i -= 1;
            }
        } else if i + 1 < layers.len() && fits(layers[i + 1], UP_ABOVE) {
            timers.worse_since = None;
            let since = *timers.better_since.get_or_insert(now);
            if now.duration_since(since) >= UP_AFTER {
                timers.better_since = None;
                i += 1;
            }
        } else {
            *timers = Timers::default();
        }
        layers[i]
    }

    /// Bitrate de uma camada: o espectador mais lento dela, com folga, entre o
    /// mínimo e o máximo da camada.
    pub fn layer_bitrate(&self, layer: u32, estimates: impl IntoIterator<Item = u64>) -> u32 {
        let max = self.rate(layer).max(MIN_BITRATE);
        match estimates.into_iter().min() {
            Some(e) => ((e * 9 / 10).min(u32::MAX as u64) as u32).clamp(MIN_BITRATE, max),
            None => max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P1080: Plan = Plan { top: 1080, fps: 60, max_bitrate: 7_200_000 };

    #[test]
    fn layers_and_rates() {
        assert_eq!(P1080.layers(), [360, 480, 720, 1080]);
        assert_eq!(Plan { top: 1032, ..P1080 }.layers(), [360, 480, 720, 1032]);
        assert_eq!(Plan { top: 720, ..P1080 }.layers(), [360, 480, 720]);
        assert_eq!(P1080.rate(1080), 7_200_000);
        assert_eq!(P1080.rate(720), 4_000_000);
        // miniatura a 30 fps
        assert_eq!(P1080.fps(480), 30);
        assert_eq!(P1080.rate(480), 1_200_000);
        assert_eq!(P1080.fps(720), 60);
    }

    #[test]
    fn wanted_matches_the_webview_rules() {
        assert_eq!(P1080.wanted(None), 1080);
        assert_eq!(P1080.wanted(Some(0)), NO_VIDEO);
        assert_eq!(P1080.wanted(Some(720)), 720);
        assert_eq!(P1080.wanted(Some(2160)), 1080);
        assert_eq!(Plan { top: 1032, ..P1080 }.wanted(Some(1080)), 1032);
    }

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn slow_viewer_goes_down_and_comes_back_after_a_while() {
        let t0 = Instant::now();
        let mut since = Timers::default();
        // começa: conexão boa, a camada inteira
        let l = P1080.choose(None, Some(20_000_000), None, &mut since, t0);
        assert_eq!(l, 1080);
        // a banda cai para 3 Mbps (< metade de 7,2): desce para 720p depois de 3 s
        let l = P1080.choose(None, Some(3_000_000), Some(l), &mut since, t0);
        assert_eq!(l, 1080);
        let l = P1080.choose(None, Some(3_000_000), Some(l), &mut since, t0 + s(3));
        assert_eq!(l, 720);
        // 2 Mbps é a metade de 4 Mbps: fica
        let l = P1080.choose(None, Some(2_000_000), Some(l), &mut since, t0 + s(9));
        assert_eq!(l, 720);
        let t0 = t0 + s(10);
        // volta a 8 Mbps: só sobe depois de 10 s
        let l = P1080.choose(None, Some(8_000_000), Some(l), &mut since, t0);
        assert_eq!(l, 720);
        let l = P1080.choose(None, Some(8_000_000), Some(l), &mut since, t0 + Duration::from_secs(5));
        assert_eq!(l, 720);
        let l = P1080.choose(None, Some(8_000_000), Some(l), &mut since, t0 + Duration::from_secs(10));
        assert_eq!(l, 1080);
    }

    #[test]
    fn a_short_dip_does_not_move_anyone_down() {
        let t0 = Instant::now();
        let mut since = Timers::default();
        let mut l = 1080;
        l = P1080.choose(None, Some(1_000_000), Some(l), &mut since, t0);
        l = P1080.choose(None, Some(1_000_000), Some(l), &mut since, t0 + s(2));
        l = P1080.choose(None, Some(9_000_000), Some(l), &mut since, t0 + s(3));
        l = P1080.choose(None, Some(1_000_000), Some(l), &mut since, t0 + s(4));
        assert_eq!(l, 1080, "the estimate came back before 3 s");
    }

    #[test]
    fn a_dip_resets_the_wait_to_go_up() {
        let t0 = Instant::now();
        let mut since = Timers::default();
        let mut l = 720;
        l = P1080.choose(None, Some(8_000_000), Some(l), &mut since, t0);
        l = P1080.choose(None, Some(4_000_000), Some(l), &mut since, t0 + Duration::from_secs(6));
        l = P1080.choose(None, Some(8_000_000), Some(l), &mut since, t0 + Duration::from_secs(11));
        assert_eq!(l, 720, "the dip restarted the 10 s");
    }

    #[test]
    fn very_slow_starts_low_and_stays_at_the_bottom() {
        let mut since = Timers::default();
        let now = Instant::now();
        assert_eq!(P1080.choose(None, Some(1_000_000), None, &mut since, now), 480);
        assert_eq!(P1080.choose(None, Some(100_000), Some(360), &mut since, now), 360);
    }

    #[test]
    fn asking_for_less_or_hiding_the_app() {
        let mut since = Timers::default();
        let now = Instant::now();
        assert_eq!(P1080.choose(Some(480), Some(20_000_000), Some(1080), &mut since, now), 480);
        assert_eq!(P1080.choose(Some(0), Some(20_000_000), Some(480), &mut since, now), NO_VIDEO);
        assert_eq!(P1080.choose(Some(1080), Some(20_000_000), Some(NO_VIDEO), &mut since, now), 1080);
    }

    #[test]
    fn layer_bitrate_follows_the_slowest_within_limits() {
        assert_eq!(P1080.layer_bitrate(1080, [20_000_000, 9_000_000]), 7_200_000);
        assert_eq!(P1080.layer_bitrate(1080, [20_000_000, 5_000_000]), 4_500_000);
        assert_eq!(P1080.layer_bitrate(360, [10_000]), MIN_BITRATE);
        assert_eq!(P1080.layer_bitrate(720, []), 4_000_000);
    }
}
