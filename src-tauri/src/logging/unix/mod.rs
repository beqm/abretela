//! O que o log precisa dos sistemas Unix (macOS e Linux): a hora local, a
//! versão do sistema, as placas de rede (`network`) e o uso do processador
//! (`perf`). O que muda de um para o outro fica em `mac.rs` e `linux.rs`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
pub mod network;
pub mod perf;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use mac as system;

pub use system::os_version;

use super::Now;

pub fn now() -> Now {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        libc::localtime_r(&t, &mut tm);
    }
    Now {
        date: format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday),
        time: format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_looks_like_a_date_and_a_time() {
        let now = now();
        let digits = |text: &str, sep: char| {
            text.split(sep).map(|part| part.parse::<u32>().expect("a number")).collect::<Vec<_>>()
        };
        let date = digits(&now.date, '-');
        let time = digits(&now.time, ':');
        assert_eq!((now.date.len(), now.time.len()), (10, 8), "{} {}", now.date, now.time);
        assert!(date[0] >= 2024 && (1..=12).contains(&date[1]) && (1..=31).contains(&date[2]));
        assert!(time[0] < 24 && time[1] < 60 && time[2] < 61);
    }
}
