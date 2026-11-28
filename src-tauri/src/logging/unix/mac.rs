//! O que é só do macOS nos logs: a versão do sistema e os nomes das placas
//! de rede.

use std::ffi::CStr;

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

/// O tipo da placa, pelo nome que o macOS dá: (texto, parece VPN ou virtual).
pub fn adapter_kind(name: &str) -> (&'static str, bool) {
    let prefix = name.trim_end_matches(|c: char| c.is_ascii_digit());
    match prefix {
        "en" => ("Ethernet/Wi-Fi", false),
        "utun" | "ipsec" | "ppp" | "gif" | "stf" | "tun" | "tap" | "wg" => ("tunnel", true),
        "bridge" | "vmnet" | "vmenet" | "vnic" | "feth" => ("virtual", true),
        "awdl" | "llw" => ("Apple wireless direct", false),
        "pdp_ip" => ("mobile", false),
        _ => ("other", false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_version_has_the_number_and_the_build() {
        let version = os_version();
        println!("{version}");
        let number = version.strip_prefix("macOS ").expect("starts with macOS");
        assert!(number.starts_with(|c: char| c.is_ascii_digit()), "{version}");
        assert!(version.contains("(build "), "{version}");
    }

    #[test]
    fn guesses_the_kind_from_the_name() {
        assert_eq!(adapter_kind("en0"), ("Ethernet/Wi-Fi", false));
        assert_eq!(adapter_kind("utun12"), ("tunnel", true));
        assert_eq!(adapter_kind("bridge100"), ("virtual", true));
        assert_eq!(adapter_kind("anpi0"), ("other", false));
    }
}
