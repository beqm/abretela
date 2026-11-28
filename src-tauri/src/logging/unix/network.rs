//! Lista as placas de rede no log (macOS e Linux): nome, tipo e IPs. Ajuda a
//! achar VPNs e redes estranhas quando ninguém consegue se conectar. (A versão
//! do Windows fica em `logging/network.rs`.)

use super::system::adapter_kind;
use std::{
    ffi::CStr,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

struct Address {
    ip: IpAddr,
    /// Tamanho do prefixo da rede (ex.: 24 em 192.168.0.5/24).
    prefix: u32,
}

struct Adapter {
    name: String,
    /// Ligada e funcionando.
    up: bool,
    loopback: bool,
    addresses: Vec<Address>,
}

fn is_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    }
}

/// IPv6 que sai para a internet (2000::/3). Os de rede interna (fd00::/8, de
/// VPNs e máquinas virtuais) não contam.
fn is_global_ipv6(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xe000 == 0x2000)
}

/// O endereço de um `sockaddr` (IPv4 ou IPv6).
unsafe fn sockaddr_to_ip(sa: *const libc::sockaddr) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    match (*sa).sa_family as i32 {
        libc::AF_INET => {
            let v4 = &*(sa as *const libc::sockaddr_in);
            Some(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr)).into())
        }
        libc::AF_INET6 => {
            let v6 = &*(sa as *const libc::sockaddr_in6);
            Some(Ipv6Addr::from(v6.sin6_addr.s6_addr).into())
        }
        _ => None,
    }
}

fn prefix_of(mask: Option<IpAddr>) -> u32 {
    match mask {
        Some(IpAddr::V4(m)) => u32::from(m).count_ones(),
        Some(IpAddr::V6(m)) => u128::from(m).count_ones(),
        None => 0,
    }
}

/// As placas de rede do sistema (`getifaddrs`), na ordem em que aparecem.
fn query() -> Result<Vec<Adapter>, std::io::Error> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut out: Vec<Adapter> = Vec::new();
    let mut p = list;
    while !p.is_null() {
        let entry = unsafe { &*p };
        p = entry.ifa_next;
        if entry.ifa_name.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy().into_owned();
        let flags = entry.ifa_flags as i32;
        // cada placa aparece uma vez por endereço (e uma sem IP, a da placa em si)
        let index = out.iter().position(|a| a.name == name).unwrap_or_else(|| {
            out.push(Adapter {
                name,
                up: flags & libc::IFF_UP != 0 && flags & libc::IFF_RUNNING != 0,
                loopback: flags & libc::IFF_LOOPBACK != 0,
                addresses: Vec::new(),
            });
            out.len() - 1
        });
        if let Some(ip) = unsafe { sockaddr_to_ip(entry.ifa_addr) } {
            let prefix = prefix_of(unsafe { sockaddr_to_ip(entry.ifa_netmask) });
            out[index].addresses.push(Address { ip, prefix });
        }
    }
    unsafe { libc::freeifaddrs(list) };
    Ok(out)
}

fn usable(adapters: &[Adapter]) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for a in adapters.iter().filter(|a| a.up && !a.loopback) {
        for address in &a.addresses {
            let ip = address.ip;
            if !ip.is_loopback() && !is_link_local(&ip) && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

/// Endereços onde dá para receber conexões: placas ligadas, sem loopback e
/// sem endereço automático (169.254, fe80). Usado pelo envio nativo para
/// criar as rotas "rede local".
pub fn usable_addresses() -> Vec<IpAddr> {
    query().map(|adapters| usable(&adapters)).unwrap_or_default()
}

/// Escreve uma linha por placa de rede ligada que tem endereço de verdade (e
/// conta as outras).
pub fn log_adapters(why: &str) {
    let adapters = match query() {
        Ok(a) => a,
        Err(e) => {
            log_warn!("network", "Could not list the network adapters ({e})");
            return;
        }
    };

    let mut lines = Vec::new();
    let mut down = Vec::new();
    let mut without_address = 0;
    let mut virtuals = Vec::new();
    let mut has_ipv6 = false;
    for a in adapters.iter().filter(|a| !a.loopback) {
        if !a.up {
            down.push(a.name.clone());
            continue;
        }
        // placas internas só com fe80 (no macOS: awdl, llw, utun do sistema)
        if a.addresses.iter().all(|address| is_link_local(&address.ip)) {
            without_address += 1;
            continue;
        }
        has_ipv6 |= a.addresses.iter().any(|address| is_global_ipv6(&address.ip));
        let (kind, looks_virtual) = adapter_kind(&a.name);
        if looks_virtual {
            virtuals.push(a.name.clone());
        }
        let ips: Vec<String> = a.addresses.iter().map(|address| format!("{}/{}", address.ip, address.prefix)).collect();
        lines.push(format!(
            "  - \"{}\": {kind}{}, up, IPs {}",
            a.name,
            if looks_virtual { ", looks like a VPN/virtual adapter" } else { "" },
            ips.join(", "),
        ));
    }

    log_info!("network", "Network adapters {why} ({} connected):", lines.len());
    for l in &lines {
        log_info!("network", "{l}");
    }
    if without_address > 0 {
        log_info!("network", "  Connected adapters without a usable address: {without_address}");
    }
    if !down.is_empty() {
        log_info!("network", "  Disconnected adapters: {}", down.join(", "));
    }
    log_info!("network", "  IPv6 on the internet: {}", if has_ipv6 { "yes" } else { "no (IPv4 only)" });
    if !virtuals.is_empty() {
        log_warn!(
            "network",
            "  VPN/virtual adapters are on ({}). They can make WebRTC pick a route the other computer cannot reach",
            virtuals.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, up: bool, loopback: bool, ips: &[&str]) -> Adapter {
        Adapter {
            name: name.into(),
            up,
            loopback,
            addresses: ips.iter().map(|ip| Address { ip: ip.parse().unwrap(), prefix: 24 }).collect(),
        }
    }

    #[test]
    fn usable_skips_loopback_link_local_and_adapters_that_are_down() {
        let adapters = [
            adapter("lo0", true, true, &["127.0.0.1", "::1"]),
            adapter("en0", true, false, &["192.168.0.5", "fe80::1c2d:3e4f:5a6b:7c8d", "2804:14c::1234"]),
            adapter("en1", false, false, &["10.0.0.9"]),
            adapter("awdl0", true, false, &["fe80::a8bb:ccff:fedd:eeff"]),
            adapter("en5", true, false, &["169.254.10.20"]),
            adapter("utun4", true, false, &["100.64.0.7", "192.168.0.5"]),
        ];
        let expected: Vec<IpAddr> =
            ["192.168.0.5", "2804:14c::1234", "100.64.0.7"].iter().map(|ip| ip.parse().unwrap()).collect();
        assert_eq!(usable(&adapters), expected);
    }

    #[test]
    fn only_global_ipv6_counts_as_internet() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        assert!(is_global_ipv6(&ip("2804:14c::1234")));
        assert!(!is_global_ipv6(&ip("fd07:b51a:cc66::1")), "internal network");
        assert!(!is_global_ipv6(&ip("fe80::1")), "link-local");
        assert!(!is_global_ipv6(&ip("192.168.0.5")));
    }

    #[test]
    fn reads_the_prefix_from_the_mask() {
        assert_eq!(prefix_of(Some("255.255.255.0".parse().unwrap())), 24);
        assert_eq!(prefix_of(Some("ffff:ffff:ffff:ffff::".parse().unwrap())), 64);
        assert_eq!(prefix_of(None), 0);
    }

    /// Lista as placas deste computador (veja com --nocapture): o loopback sempre
    /// existe, e nenhum endereço utilizável é loopback ou automático.
    #[test]
    fn lists_the_adapters_of_this_computer() {
        log_adapters("in a test");
        let adapters = query().expect("getifaddrs works");
        assert!(adapters.iter().any(|a| a.loopback), "the loopback adapter is there");
        let usable = usable_addresses();
        println!("usable: {usable:?}");
        assert!(usable.iter().all(|ip| !ip.is_loopback() && !is_link_local(ip)));
    }
}
