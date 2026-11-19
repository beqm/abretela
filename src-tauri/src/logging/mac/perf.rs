//! Quanto o Mac gasta enquanto transmite: a cada 30 s, enquanto a captura
//! roda, uma linha no log com o processador usado pelo app, em % do Mac
//! inteiro. (A versão do Windows, em `logging/perf.rs`, também mede o WebView
//! e a placa de vídeo; no macOS os processos do WebKit não são filhos do app e
//! o sistema não dá o uso da placa por processo.)

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const EVERY: Duration = Duration::from_secs(30);

/// Começa a medir até `stop`. `mode` entra no texto (ex.: "native sending").
pub fn watch(mode: &'static str, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("perf".into())
        .spawn(move || {
            let mut cpu = CpuMeter::default();
            cpu.sample();
            let mut next = Instant::now() + EVERY;
            while !stop.load(Ordering::Relaxed) {
                if Instant::now() < next {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
                next += EVERY;
                log_info!(
                    "perf",
                    "While sharing ({mode}): processor {:.1}% of the Mac (app only; the WebView and the graphics chip are not measured on macOS)",
                    cpu.sample()
                );
            }
        })
        .ok();
}

#[derive(Default)]
struct CpuMeter {
    /// Tempo de processador já usado pelo app na última vez.
    last: Duration,
    at: Option<Instant>,
}

impl CpuMeter {
    /// % do Mac inteiro que o app usou desde a última chamada.
    fn sample(&mut self) -> f64 {
        let now = Instant::now();
        let used = cpu_time();
        let delta = used.saturating_sub(self.last);
        let wall = self.at.map(|a| now.duration_since(a)).unwrap_or_default();
        self.last = used;
        self.at = Some(now);
        let cores = thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let total = wall.as_secs_f64() * cores;
        if total <= 0.0 {
            return 0.0;
        }
        delta.as_secs_f64() * 100.0 / total
    }
}

/// Tempo de processador (usuário + sistema) que este processo já usou.
fn cpu_time() -> Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return Duration::ZERO;
    }
    let of = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    of(usage.ru_utime) + of(usage.ru_stime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_the_processor_this_process_uses() {
        let mut cpu = CpuMeter::default();
        assert_eq!(cpu.sample(), 0.0, "nothing to compare with on the first call");
        let before = cpu_time();
        let end = Instant::now() + Duration::from_millis(200);
        let mut x = 0u64;
        while Instant::now() < end {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
        }
        assert!(cpu_time() - before >= Duration::from_millis(100), "a busy loop uses the processor");
        let percent = cpu.sample();
        println!("busy loop result {x}, {percent:.1}% of the Mac");
        assert!(percent > 0.0 && percent <= 100.0, "{percent}");
    }
}
