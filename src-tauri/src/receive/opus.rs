//! Pacotes Opus que chegam partidos.
//!
//! Um pacote Opus tem de caber inteiro num pacote da rede, mas o `str0m` de
//! quem envia parte em dois o que passa do limite dele (visto: 1120 + 116
//! bytes, num som forte e repentino a 128 kbps). Cada pedaço chega aqui como
//! se fosse um pacote, com o mesmo tempo do outro. Um navegador toca o
//! primeiro pedaço como um estalo e segue; o decodificador do WebKitGTK para
//! de vez no segundo ("corrupted stream"). Por isso os pedaços são juntados
//! de volta antes de irem para o WebView.

use std::{sync::Arc, time::Duration};

/// O maior pacote Opus que existe (RFC 6716: 1275 bytes por frame).
const LARGEST: usize = 1275;
/// A partir de quanto um pacote pode ser só o primeiro pedaço de um maior:
/// abaixo de qualquer limite de pacote de rede que o envio use. A 128 kbps um
/// pacote tem uns 320 bytes; perto de 1000 é raro.
const MAYBE_SPLIT: usize = 1000;

/// Junta de volta os pacotes Opus partidos. Um pacote grande espera o
/// seguinte chegar (20 ms) para se saber se era um pedaço.
#[derive(Default)]
pub struct Rejoin {
    held: Option<(Duration, Vec<u8>)>,
    /// Quantos pacotes foram juntados, para o log.
    pub joined: u64,
}

impl Rejoin {
    /// Entra o que chegou, com o tempo dele; saem os pacotes inteiros que
    /// ficaram prontos, na ordem.
    pub fn push(&mut self, timestamp: Duration, data: &[u8]) -> Vec<(Duration, Arc<[u8]>)> {
        let mut ready = Vec::new();
        if let Some((held_at, mut held)) = self.held.take() {
            if held_at == timestamp && held.len() + data.len() <= LARGEST {
                // o resto do pacote que estava esperando
                held.extend_from_slice(data);
                self.joined += 1;
                ready.push((held_at, held.into()));
                return ready;
            }
            // não era um pedaço: era um pacote grande, inteiro
            ready.push((held_at, held.into()));
        }
        if data.len() >= MAYBE_SPLIT {
            self.held = Some((timestamp, data.to_vec()));
        } else {
            ready.push((timestamp, data.into()));
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// O que saiu, como (tempo em ms, tamanho).
    fn sizes(out: &[(Duration, Arc<[u8]>)]) -> Vec<(u128, usize)> {
        out.iter().map(|(t, d)| (t.as_millis(), d.len())).collect()
    }

    #[test]
    fn ordinary_packets_go_straight_through() {
        let mut rejoin = Rejoin::default();
        assert_eq!(sizes(&rejoin.push(at(0), &[1; 5])), [(0, 5)]);
        assert_eq!(sizes(&rejoin.push(at(20), &[1; 320])), [(20, 320)]);
        assert_eq!(sizes(&rejoin.push(at(40), &[1; 871])), [(40, 871)]);
        assert_eq!(rejoin.joined, 0);
    }

    #[test]
    fn the_two_pieces_of_a_split_packet_come_out_as_one() {
        // o que a VM recebeu do Mac: 1120 e 116 bytes com o mesmo tempo
        let mut rejoin = Rejoin::default();
        assert!(rejoin.push(at(14_400), &[7; 1120]).is_empty(), "waits to see if the rest comes");
        let out = rejoin.push(at(14_400), &[9; 116]);
        assert_eq!(sizes(&out), [(14_400, 1236)]);
        assert_eq!(&out[0].1[1118..1122], [7, 7, 9, 9], "the first piece, then the second");
        assert_eq!(rejoin.joined, 1);
        // e a vida segue
        assert_eq!(sizes(&rejoin.push(at(14_420), &[1; 400])), [(14_420, 400)]);
    }

    #[test]
    fn a_large_packet_that_was_whole_leaves_when_the_next_one_arrives() {
        let mut rejoin = Rejoin::default();
        assert!(rejoin.push(at(100), &[1; 1050]).is_empty());
        assert_eq!(sizes(&rejoin.push(at(120), &[1; 300])), [(100, 1050), (120, 300)], "in order, nothing lost");
        // dois grandes seguidos: o primeiro sai, o segundo espera
        assert!(rejoin.push(at(140), &[1; 1100]).is_empty());
        assert_eq!(sizes(&rejoin.push(at(160), &[1; 1100])), [(140, 1100)]);
        assert_eq!(sizes(&rejoin.push(at(180), &[1; 10])), [(160, 1100), (180, 10)]);
        assert_eq!(rejoin.joined, 0);
    }

    #[test]
    fn pieces_that_would_not_fit_in_one_packet_are_not_joined() {
        // o mesmo tempo duas vezes, mas grande demais para ser um pacote só
        let mut rejoin = Rejoin::default();
        assert!(rejoin.push(at(0), &[1; 1200]).is_empty());
        assert_eq!(sizes(&rejoin.push(at(0), &[1; 200])), [(0, 1200), (0, 200)]);
        assert_eq!(rejoin.joined, 0);
    }
}
