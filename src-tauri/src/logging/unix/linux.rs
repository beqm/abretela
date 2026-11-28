//! O que é só do Linux nos logs: a versão do sistema e os nomes das placas
//! de rede.

/// Ex.: "Ubuntu 24.04.5 LTS (kernel 6.8.0-87-generic)".
pub fn os_version() -> String {
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut out = pretty_name(&release).unwrap_or_else(|| "Linux".into());
    if let Ok(kernel) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        out.push_str(&format!(" (kernel {})", kernel.trim()));
    }
    out
}

/// O nome da distribuição no `/etc/os-release` (linha `PRETTY_NAME="..."`).
fn pretty_name(os_release: &str) -> Option<String> {
    let value = os_release.lines().find_map(|line| line.strip_prefix("PRETTY_NAME="))?;
    let name = value.trim().trim_matches('"');
    (!name.is_empty()).then(|| name.to_string())
}

/// O tipo da placa, pelo nome que o Linux dá: (texto, parece VPN ou virtual).
pub fn adapter_kind(name: &str) -> (&'static str, bool) {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| name.starts_with(p));
    if starts(&["wl"]) {
        ("Wi-Fi", false)
    } else if starts(&["en", "eth"]) {
        ("Ethernet", false)
    } else if starts(&["tun", "tap", "wg", "tailscale", "zt", "ppp", "nordlynx", "proton"]) {
        ("tunnel", true)
    } else if starts(&["docker", "br-", "veth", "virbr", "vmnet", "vboxnet", "lxc", "podman"]) {
        ("virtual", true)
    } else if starts(&["ww"]) {
        ("mobile", false)
    } else {
        ("other", false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_name_of_the_distribution() {
        let file = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.5 LTS\"\nID=ubuntu\n";
        assert_eq!(pretty_name(file).as_deref(), Some("Ubuntu 24.04.5 LTS"));
        assert_eq!(pretty_name("ID=arch\nPRETTY_NAME=Arch Linux\n").as_deref(), Some("Arch Linux"));
        assert_eq!(pretty_name("ID=x\n"), None);
    }

    #[test]
    fn os_version_has_the_distribution_and_the_kernel() {
        let version = os_version();
        println!("{version}");
        assert!(!version.starts_with(" ") && version.contains("(kernel "), "{version}");
    }

    #[test]
    fn guesses_the_kind_from_the_name() {
        assert_eq!(adapter_kind("enp3s0"), ("Ethernet", false));
        assert_eq!(adapter_kind("eth0"), ("Ethernet", false));
        assert_eq!(adapter_kind("wlp2s0"), ("Wi-Fi", false));
        assert_eq!(adapter_kind("tailscale0"), ("tunnel", true));
        assert_eq!(adapter_kind("docker0"), ("virtual", true));
        assert_eq!(adapter_kind("dummy0"), ("other", false));
    }
}
