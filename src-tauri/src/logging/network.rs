//! Lista as placas de rede do PC no log: nome, tipo, se está ligada, IPs,
//! gateway, MTU e velocidade. Ajuda a achar VPNs (Radmin, Hamachi, ZeroTier,
//! Tailscale...) e redes estranhas quando ninguém consegue se conectar.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR},
    NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    },
    Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6, SOCKET_ADDRESS},
};

/// Nomes que costumam ser VPN ou rede virtual (minúsculas).
const VIRTUAL_HINTS: &[&str] = &[
    "radmin", "hamachi", "zerotier", "tailscale", "wireguard", "openvpn", "tap-windows", "tap adapter",
    "wintun", "nordlynx", "nordvpn", "expressvpn", "protonvpn", "surfshark", "cloudflare warp", "warp",
    "fortinet", "forticlient", "cisco anyconnect", "globalprotect", "softether", "vpn", "virtualbox",
    "vmware", "hyper-v", "vethernet", "wsl", "npcap", "loopback",
];

fn wide_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

fn sockaddr_to_ip(a: &SOCKET_ADDRESS) -> Option<IpAddr> {
    let sa = a.lpSockaddr as *const SOCKADDR;
    if sa.is_null() {
        return None;
    }
    unsafe {
        match (*sa).sa_family {
            AF_INET => {
                let v4 = &*(sa as *const SOCKADDR_IN);
                Some(Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr)).into())
            }
            AF_INET6 => {
                let v6 = &*(sa as *const SOCKADDR_IN6);
                Some(Ipv6Addr::from(v6.sin6_addr.u.Byte).into())
            }
            _ => None,
        }
    }
}

fn sockaddr_to_string(a: &SOCKET_ADDRESS) -> Option<String> {
    sockaddr_to_ip(a).map(|ip| ip.to_string())
}

fn if_type(t: u32) -> &'static str {
    match t {
        6 => "Ethernet",
        71 => "Wi-Fi",
        23 | 53 => "virtual/PPP",
        24 => "loopback",
        131 => "tunnel",
        243 | 244 => "mobile",
        _ => "other",
    }
}

fn oper_status(s: i32) -> &'static str {
    match s {
        1 => "up",
        2 => "down",
        3 => "testing",
        5 => "dormant",
        6 => "not present",
        7 => "lower layer down",
        _ => "unknown",
    }
}

fn speed(bits: u64) -> String {
    match bits {
        b if b == u64::MAX || b < 1_000_000 => "?".into(),
        b if b >= 1_000_000_000 => format!("{} Gbps", b / 1_000_000_000),
        b => format!("{} Mbps", b / 1_000_000),
    }
}

fn is_link_local(ip: &str) -> bool {
    ip.starts_with("169.254.") || ip.starts_with("fe80:")
}

/// Teredo (2001:0::/32): IPv6 "de mentira" por cima do IPv4, quase nunca conecta.
fn is_teredo(ip: &str) -> bool {
    ip.starts_with("2001:0:") || ip.starts_with("2001::")
}

/// A lista crua do Windows (as estruturas apontam para dentro do vetor).
fn query() -> Result<Vec<u64>, u32> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u64> = Vec::new(); // u64 para alinhar a estrutura
    let mut rc = ERROR_BUFFER_OVERFLOW;
    for _ in 0..3 {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        rc = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                flags,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        if rc != ERROR_BUFFER_OVERFLOW {
            break;
        }
    }
    if rc != NO_ERROR {
        return Err(rc);
    }
    Ok(buf)
}

/// Endereços onde dá para receber conexões: placas ligadas, sem loopback,
/// sem endereço automático (169.254, fe80) e sem Teredo. Usado pelo envio
/// nativo para criar as rotas "rede local".
pub fn usable_addresses() -> Vec<IpAddr> {
    let Ok(buf) = query() else { return Vec::new() };
    let mut out = Vec::new();
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        let a = unsafe { &*p };
        p = a.Next;
        if a.IfType == 24 || a.OperStatus != 1 {
            continue;
        }
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let ua = unsafe { &*u };
            if let Some(ip) = sockaddr_to_ip(&ua.Address) {
                let text = ip.to_string();
                if !ip.is_loopback() && !is_link_local(&text) && !is_teredo(&text) && !out.contains(&ip) {
                    out.push(ip);
                }
            }
            u = ua.Next;
        }
    }
    out
}

/// Escreve uma linha por placa de rede ligada (e conta as desligadas).
pub fn log_adapters(why: &str) {
    let buf = match query() {
        Ok(b) => b,
        Err(rc) => {
            log_warn!("network", "Could not list the network adapters (error {rc})");
            return;
        }
    };

    let mut lines = Vec::new();
    let mut down = Vec::new();
    let mut virtuals = Vec::new();
    let mut has_ipv6 = false;
    let mut teredo = false;
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        let a = unsafe { &*p };
        p = a.Next;
        let name = wide_to_string(a.FriendlyName);
        let desc = wide_to_string(a.Description);
        let kind = if_type(a.IfType);
        if a.IfType == 24 {
            continue; // loopback não interessa
        }
        let status = oper_status(a.OperStatus);
        if a.OperStatus != 1 {
            down.push(name);
            continue;
        }

        let mut ips = Vec::new();
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let ua = unsafe { &*u };
            if let Some(ip) = sockaddr_to_string(&ua.Address) {
                if ip.contains(':') && !is_link_local(&ip) {
                    if is_teredo(&ip) {
                        teredo = true;
                    } else {
                        has_ipv6 = true;
                    }
                }
                ips.push(format!("{ip}/{}", ua.OnLinkPrefixLength));
            }
            u = ua.Next;
        }
        let mut gateways = Vec::new();
        let mut g = a.FirstGatewayAddress;
        while !g.is_null() {
            let ga = unsafe { &*g };
            if let Some(ip) = sockaddr_to_string(&ga.Address).filter(|ip| ip != "::" && ip != "0.0.0.0") {
                gateways.push(ip);
            }
            g = ga.Next;
        }

        let haystack = format!("{name} {desc}").to_lowercase();
        let looks_virtual = a.IfType == 53 || a.IfType == 131 || VIRTUAL_HINTS.iter().any(|h| haystack.contains(h));
        if looks_virtual {
            virtuals.push(name.clone());
        }
        lines.push(format!(
            "  - \"{name}\" ({desc}): {kind}{}, {status}, IPs {}, gateway {}, MTU {}, speed {}",
            if looks_virtual { ", looks like a VPN/virtual adapter" } else { "" },
            if ips.is_empty() { "none".into() } else { ips.join(", ") },
            if gateways.is_empty() { "none".into() } else { gateways.join(", ") },
            a.Mtu,
            speed(a.TransmitLinkSpeed),
        ));
    }

    log_info!("network", "Network adapters {why} ({} connected):", lines.len());
    for l in &lines {
        log_info!("network", "{l}");
    }
    if !down.is_empty() {
        log_info!("network", "  Disconnected adapters: {}", down.join(", "));
    }
    let ipv6 = match (has_ipv6, teredo) {
        (true, _) => "yes",
        (false, true) => "only through Teredo (a Windows tunnel that rarely works for calls; effectively IPv4 only)",
        (false, false) => "no (IPv4 only)",
    };
    log_info!("network", "  IPv6 on the internet: {ipv6}");
    if !virtuals.is_empty() {
        log_warn!(
            "network",
            "  VPN/virtual adapters are on ({}). They can make WebRTC pick a route the other PC cannot reach",
            virtuals.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    /// Lista as placas deste PC (só confere que não quebra; veja com --nocapture).
    #[test]
    fn lists_adapters_without_crashing() {
        super::log_adapters("in a test");
        println!("usable: {:?}", super::usable_addresses());
    }
}
