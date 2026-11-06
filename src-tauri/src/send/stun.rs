//! STUN mínimo (RFC 5389): pergunta a um servidor público "de onde você me
//! vê?" e lê a resposta (XOR-MAPPED-ADDRESS). É assim que o envio nativo
//! descobre o IP e a porta públicos de cada socket (a rota "public address"),
//! já que o str0m não faz isso sozinho.

use std::{
    hash::{BuildHasher, RandomState},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Instant,
};

/// Os mesmos servidores do WebRTC do WebView (`mesh.ts`).
pub const SERVERS: &[&str] = &["stun.cloudflare.com:3478", "stun.l.google.com:19302"];

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const ATTR_MAPPED: u16 = 0x0001;
const ATTR_XOR_MAPPED: u16 = 0x0020;
/// Começo dos nossos ids de transação, para separar as nossas respostas das
/// checagens do ICE (que também são STUN e vão para o str0m).
const OUR_PREFIX: &[u8; 4] = b"AbTl";

pub fn new_transaction() -> [u8; 12] {
    let r = RandomState::new().hash_one(Instant::now()).to_le_bytes();
    let mut id = [0u8; 12];
    id[..4].copy_from_slice(OUR_PREFIX);
    id[4..].copy_from_slice(&r);
    id
}

/// Pedido "Binding" sem atributos.
pub fn binding_request(transaction: [u8; 12]) -> [u8; 20] {
    let mut p = [0u8; 20];
    p[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    p[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    p[8..20].copy_from_slice(&transaction);
    p
}

/// Se o pacote é a resposta a um pedido nosso, devolve o endereço público.
/// `None` para qualquer outra coisa (inclusive checagens do ICE).
pub fn parse_our_response(p: &[u8]) -> Option<SocketAddr> {
    if p.len() < 20
        || u16::from_be_bytes([p[0], p[1]]) != BINDING_SUCCESS
        || u32::from_be_bytes([p[4], p[5], p[6], p[7]]) != MAGIC
        || &p[8..12] != OUR_PREFIX
    {
        return None;
    }
    let len = u16::from_be_bytes([p[2], p[3]]) as usize;
    let body = p.get(20..20 + len)?;
    let transaction = &p[8..20];
    let mut plain = None;
    let mut i = 0;
    while i + 4 <= body.len() {
        let kind = u16::from_be_bytes([body[i], body[i + 1]]);
        let alen = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
        let value = body.get(i + 4..i + 4 + alen)?;
        match kind {
            ATTR_XOR_MAPPED => return read_address(value, Some(transaction)),
            ATTR_MAPPED => plain = read_address(value, None),
            _ => {}
        }
        i += 4 + alen.div_ceil(4) * 4; // atributos alinhados em 4 bytes
    }
    plain
}

fn read_address(v: &[u8], xor: Option<&[u8]>) -> Option<SocketAddr> {
    if v.len() < 8 {
        return None;
    }
    let magic = MAGIC.to_be_bytes();
    let mut port = u16::from_be_bytes([v[2], v[3]]);
    if xor.is_some() {
        port ^= (MAGIC >> 16) as u16;
    }
    let ip: IpAddr = match v[1] {
        0x01 => {
            let mut b = [v[4], v[5], v[6], v[7]];
            if xor.is_some() {
                b.iter_mut().zip(magic).for_each(|(x, m)| *x ^= m);
            }
            Ipv4Addr::from(b).into()
        }
        0x02 if v.len() >= 20 => {
            let mut b: [u8; 16] = v[4..20].try_into().ok()?;
            if let Some(t) = xor {
                let key: Vec<u8> = magic.iter().chain(t).copied().collect();
                b.iter_mut().zip(key).for_each(|(x, k)| *x ^= k);
            }
            Ipv6Addr::from(b).into()
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta uma resposta como a de um servidor STUN.
    fn response(transaction: [u8; 12], public: SocketAddr) -> Vec<u8> {
        let magic = MAGIC.to_be_bytes();
        let mut attr = vec![0, 0];
        attr.extend((public.port() ^ (MAGIC >> 16) as u16).to_be_bytes());
        match public.ip() {
            IpAddr::V4(ip) => {
                attr[1] = 1;
                attr.extend(ip.octets().iter().zip(magic).map(|(a, m)| a ^ m));
            }
            IpAddr::V6(ip) => {
                attr[1] = 2;
                let key: Vec<u8> = magic.iter().chain(&transaction).copied().collect();
                attr.extend(ip.octets().iter().zip(key).map(|(a, k)| a ^ k));
            }
        }
        let mut p = Vec::new();
        p.extend(BINDING_SUCCESS.to_be_bytes());
        p.extend(((attr.len() + 4) as u16).to_be_bytes());
        p.extend(magic);
        p.extend(transaction);
        p.extend(ATTR_XOR_MAPPED.to_be_bytes());
        p.extend((attr.len() as u16).to_be_bytes());
        p.extend(attr);
        p
    }

    #[test]
    fn reads_ipv4_and_ipv6() {
        for public in ["189.1.2.3:54321", "[2804:14c::1]:6000"] {
            let public: SocketAddr = public.parse().unwrap();
            let t = new_transaction();
            assert_eq!(parse_our_response(&response(t, public)), Some(public));
        }
    }

    #[test]
    fn ignores_ice_checks_and_garbage() {
        let mut other = response([7; 12], "1.2.3.4:5".parse().unwrap());
        assert_eq!(parse_our_response(&other), None, "not our transaction");
        other.truncate(10);
        assert_eq!(parse_our_response(&other), None);
        assert_eq!(parse_our_response(&binding_request(new_transaction())), None, "a request, not a response");
    }

    /// `cargo test stun_live -- --ignored --nocapture` (precisa de internet)
    #[test]
    #[ignore]
    fn stun_live() {
        use std::net::{ToSocketAddrs, UdpSocket};
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        socket.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        for server in SERVERS {
            let addr = server.to_socket_addrs().unwrap().find(|a| a.is_ipv4()).unwrap();
            socket.send_to(&binding_request(new_transaction()), addr).unwrap();
            let mut buf = [0u8; 512];
            let (n, _) = socket.recv_from(&mut buf).unwrap();
            let public = parse_our_response(&buf[..n]);
            println!("{server}: {public:?} (local {})", socket.local_addr().unwrap());
            assert!(public.is_some());
        }
    }
}
