//! O que o log precisa do macOS: a hora local, a versão do sistema, as placas
//! de rede (`network`) e o uso do processador (`perf`).

pub mod network;
pub mod perf;

use super::Now;
use std::ffi::CStr;

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

/// Um texto do `sysctl` (ex.: `kern.osproductversion`).
fn sysctl_string(name: &CStr) -> Option<String> {
    let mut buf = [0u8; 128];
    let mut size = buf.len();
    let rc = unsafe {
        libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0)
    };
    if rc != 0 {
        return None;
    }
    let text = CStr::from_bytes_until_nul(&buf).ok()?.to_str().ok()?;
    (!text.is_empty()).then(|| text.to_string())
}

/// Ex.: "macOS 15.1 (build 24B83)".
pub fn os_version() -> String {
    let mut out = "macOS".to_string();
    if let Some(version) = sysctl_string(c"kern.osproductversion") {
        out.push(' ');
        out.push_str(&version);
    }
    if let Some(build) = sysctl_string(c"kern.osversion") {
        out.push_str(&format!(" (build {build})"));
    }
    out
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

    #[test]
    fn os_version_has_the_number_and_the_build() {
        let version = os_version();
        println!("{version}");
        let number = version.strip_prefix("macOS ").expect("starts with macOS");
        assert!(number.starts_with(|c: char| c.is_ascii_digit()), "{version}");
        assert!(version.contains("(build "), "{version}");
    }
}
