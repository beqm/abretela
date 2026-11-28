//! A recepção nativa conversando com um envio de verdade na mesma máquina:
//! um `Rtc` do str0m que manda o H.264 de `fixture.h264` (18 frames, 2
//! quadros-chave) e pacotes de som por UDP em 127.0.0.1, com a sinalização
//! passando por canais. É o WebRTC inteiro (rotas, criptografia, pacotes),
//! sem captura nem codificador, então roda em qualquer sistema.

use super::watcher::{first_mid, without_hidden_routes};
use super::*;
use std::{
    net::UdpSocket,
    time::{Duration, Instant},
};
use str0m::{
    change::SdpAnswer,
    format::Codec,
    media::{Direction, Frequency, MediaKind, MediaTime},
    net::{Protocol, Receive},
    Candidate, Event, Input, Output, Rtc,
};

const FIXTURE: &[u8] = include_bytes!("fixture.h264");
/// Delimitador de unidade de acesso: marca o começo de cada frame no fixture.
const NAL_DELIMITER: u8 = 9;

/// Os NALs de um trecho em Annex B (sem os códigos de início).
fn nals(annex_b: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new(); // (onde o código começa, onde o NAL começa)
    let mut i = 0;
    while i + 3 <= annex_b.len() {
        if annex_b[i..i + 3] == [0, 0, 1] {
            let code = if i > 0 && annex_b[i - 1] == 0 { i - 1 } else { i };
            starts.push((code, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &(_, from))| &annex_b[from..starts.get(n + 1).map_or(annex_b.len(), |&(code, _)| code)])
        .filter(|nal| !nal.is_empty())
        .collect()
}

/// O conteúdo de um frame, sem os delimitadores (que o transporte pode tirar).
fn content(annex_b: &[u8]) -> Vec<&[u8]> {
    nals(annex_b).into_iter().filter(|nal| nal[0] & 0x1f != NAL_DELIMITER).collect()
}

/// Os frames do fixture, cada um em Annex B.
fn access_units(annex_b: &[u8]) -> Vec<Vec<u8>> {
    let mut units: Vec<Vec<u8>> = Vec::new();
    for nal in nals(annex_b) {
        if nal[0] & 0x1f == NAL_DELIMITER || units.is_empty() {
            units.push(Vec::new());
        }
        let unit = units.last_mut().unwrap();
        unit.extend_from_slice(&[0, 0, 0, 1]);
        unit.extend_from_slice(nal);
    }
    units
}

struct Wire(mpsc::Sender<Outgoing>);

impl Signals for Wire {
    fn send(&self, _sharer: &str, msg: Outgoing) {
        let _ = self.0.send(msg);
    }
}

#[derive(Default)]
struct Collect {
    video: Mutex<Vec<Received>>,
    audio: Mutex<Vec<Received>>,
}

impl MediaSink for Collect {
    fn video(&self, frame: Received) {
        self.video.lock().unwrap().push(frame);
    }
    fn audio(&self, packet: Received) {
        self.audio.lock().unwrap().push(packet);
    }
}

#[test]
fn splits_the_fixture_into_its_frames() {
    let units = access_units(FIXTURE);
    assert_eq!(units.len(), 18);
    assert_eq!(units.iter().filter(|u| crate::encode::has_idr(u)).count(), 2);
    assert!(crate::encode::has_idr(&units[0]), "starts with a keyframe");
}

#[test]
fn drops_the_routes_the_browser_hid_and_finds_the_first_mid() {
    let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:video0\r\n\
               a=candidate:1 1 udp 2113937151 5a073f61-3aaa-4189-a910-be1a08e287c2.local 49562 typ host\r\n\
               a=candidate:2 1 udp 1677729535 203.0.113.7 49562 typ srflx raddr 0.0.0.0 rport 0\r\n\
               m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:audio1\r\n";
    let clean = without_hidden_routes(sdp);
    assert!(!clean.contains(".local"), "the hidden route is gone");
    assert!(clean.contains("203.0.113.7") && clean.contains("a=mid:audio1\r\n"), "the rest stays as it was");
    assert_eq!(first_mid(sdp), "video0");
    assert_eq!(first_mid("v=0\r\n"), "0");
}

#[test]
fn receives_what_a_sender_on_this_machine_sends() {
    // quem transmite: um str0m mandando o fixture
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(Duration::from_millis(2))).unwrap();
    let local = socket.local_addr().unwrap();
    let mut rtc = Rtc::builder().clear_codecs().enable_h264(true).enable_opus(true, false).build(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut api = rtc.sdp_api();
    let video = api.add_media(MediaKind::Video, Direction::SendOnly, Some("abretela".into()), Some("screen".into()), None);
    let audio = api.add_media(MediaKind::Audio, Direction::SendOnly, Some("abretela".into()), Some("sound".into()), None);
    let (offer, pending) = api.apply().unwrap();
    let mut pending = Some(pending);

    // quem assiste: a recepção nativa, escutando só em 127.0.0.1
    let (to_sender, from_watcher) = mpsc::channel();
    let sink = Arc::new(Collect::default());
    open_on(
        "sharer",
        "\"Sharer\"",
        &offer.to_sdp_string(),
        vec!["127.0.0.1".parse().unwrap()],
        Arc::new(Wire(to_sender)),
        sink.clone(),
    )
    .unwrap();

    let units = access_units(FIXTURE);
    let mut connected: Option<Instant> = None;
    let (mut sent, mut sent_audio) = (0usize, 0u64);
    let mut states = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buf = vec![0u8; 2048];
    while Instant::now() < deadline && sink.video.lock().unwrap().len() < units.len() {
        // a resposta e as rotas de quem assiste
        while let Ok(msg) = from_watcher.try_recv() {
            match msg {
                Outgoing::Answer { sdp } => {
                    let answer = SdpAnswer::from_sdp_string(&sdp).unwrap();
                    rtc.sdp_api().accept_answer(pending.take().unwrap(), answer).unwrap();
                }
                Outgoing::Ice { candidate, .. } => {
                    rtc.add_remote_candidate(Candidate::from_sdp_string(&candidate).unwrap());
                }
                Outgoing::State { state } => states.push(state),
            }
        }
        loop {
            match rtc.poll_output().unwrap() {
                Output::Timeout(_) => break,
                Output::Transmit(t) => {
                    socket.send_to(&t.contents, t.destination).unwrap();
                }
                Output::Event(Event::Connected) => connected = Some(Instant::now()),
                Output::Event(_) => {}
            }
        }
        // um frame a cada 33 ms e um pacote de som a cada 20 ms
        if let Some(since) = connected {
            let elapsed = since.elapsed();
            if sent < units.len() && elapsed >= Duration::from_millis(33) * sent as u32 {
                let writer = rtc.writer(video).unwrap();
                let pt = writer.payload_params().find(|p| p.spec().codec == Codec::H264).unwrap().pt();
                let time = MediaTime::new(sent as u64 * 3000, Frequency::NINETY_KHZ);
                writer.write(pt, Instant::now(), time, units[sent].clone()).unwrap();
                sent += 1;
            }
            if elapsed >= Duration::from_millis(20) * sent_audio as u32 {
                let writer = rtc.writer(audio).unwrap();
                let pt = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).unwrap().pt();
                let time = MediaTime::new(sent_audio * 960, Frequency::FORTY_EIGHT_KHZ);
                // um pacote Opus mínimo (só o cabeçalho): o transporte não olha dentro
                writer.write(pt, Instant::now(), time, vec![0xF8, 0xFF, 0xFE]).unwrap();
                sent_audio += 1;
            }
        }
        match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                let contents = buf[..n].try_into().unwrap();
                let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents });
                if rtc.accepts(&input) {
                    rtc.handle_input(input).unwrap();
                }
            }
            Err(_) => rtc.handle_input(Input::Timeout(Instant::now())).unwrap(),
        }
    }
    close("sharer");

    let frames = sink.video.lock().unwrap();
    let packets = sink.audio.lock().unwrap();
    assert_eq!(frames.len(), units.len(), "every frame arrived (sent {sent}, states {states:?})");
    assert!(frames[0].keyframe, "starts with a keyframe");
    assert_eq!(frames.iter().filter(|f| f.keyframe).count(), 2, "both keyframes are marked");
    assert!(frames.windows(2).all(|pair| pair[0].timestamp < pair[1].timestamp), "in order");
    for (n, (frame, unit)) in frames.iter().zip(&units).enumerate() {
        assert_eq!(content(&frame.data), content(unit), "frame {n} arrived whole");
    }
    // 18 frames a 30 fps = 0,6 s: o tempo do último segue o de quem mandou
    let last = frames.last().unwrap().timestamp.as_millis();
    assert!((500..700).contains(&last), "the last frame is at {last} ms");
    assert!(packets.len() > 10, "audio arrived too ({} packets)", packets.len());
    assert!(packets.iter().all(|p| &p.data[..] == [0xF8, 0xFF, 0xFE]), "audio packets arrived whole");
    assert!(states.contains(&"connected".to_string()), "the app is told it connected: {states:?}");
}

/// Conta o que chega (e, com `WATCH_DUMP`, guarda o H.264 para conferir no ffmpeg).
#[derive(Default)]
struct Count {
    frames: Mutex<Vec<(bool, usize, Duration)>>,
    audio: Mutex<u64>,
    dump: Mutex<Vec<u8>>,
}

impl MediaSink for Count {
    fn video(&self, frame: Received) {
        self.frames.lock().unwrap().push((frame.keyframe, frame.data.len(), frame.timestamp));
        if std::env::var("WATCH_DUMP").is_ok() {
            self.dump.lock().unwrap().extend_from_slice(&frame.data);
        }
    }
    fn audio(&self, _packet: Received) {
        *self.audio.lock().unwrap() += 1;
    }
}

/// Manual: assiste um envio nativo de verdade (o teste `native_send_browser`,
/// nesta máquina ou em outra) fazendo o papel da página dele: a mesma
/// sinalização por HTTP, mas quem recebe é a recepção nativa.
///
/// Num terminal: `NATIVE_SYNTH=1 cargo test native_send_browser -- --ignored --nocapture`
/// Em outro (ou em outra máquina, com `WATCH_URL=http://<ip>:3011`):
/// `cargo test native_watch_http -- --ignored --nocapture`
/// (`WATCH_SECS`, padrão 10; `WATCH_DUMP=arquivo.h264`; `WATCH_ONLY=v4` ou `v6`)
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn native_watch_http() {
    let base = std::env::var("WATCH_URL").unwrap_or_else(|_| "http://127.0.0.1:3011".into());
    let secs: u64 = std::env::var("WATCH_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    let id = "viewer-native";
    // o mesmo TLS que o resto do app usa (o cliente precisa de um escolhido, mesmo para http)
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::new();
    let post = |path: &str, body: serde_json::Value| {
        let request = http.post(format!("{base}/{path}?id={id}")).body(body.to_string());
        async move { request.send().await.expect("the sender is reachable").text().await.unwrap() }
    };

    let (to_sender, from_watcher) = mpsc::channel();
    let signals: Arc<dyn Signals> = Arc::new(Wire(to_sender));
    let sink = Arc::new(Count::default());
    println!("asking {base} to watch: {}", post("watch", serde_json::json!({})).await);

    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut states = Vec::new();
    while Instant::now() < deadline {
        // o que quem transmite mandou (a oferta e as rotas dele)
        let events = http.get(format!("{base}/events?id={id}")).send().await.unwrap().text().await.unwrap();
        for msg in serde_json::from_str::<Vec<serde_json::Value>>(&events).unwrap_or_default() {
            match msg["kind"].as_str() {
                Some("offer") => {
                    // WATCH_ONLY=v4 ou v6: escuta só num tipo de endereço (para achar por onde a conexão fecha)
                    let only = std::env::var("WATCH_ONLY").unwrap_or_default();
                    let addresses: Vec<_> = crate::logging::network::usable_addresses()
                        .into_iter()
                        .filter(|ip| match only.as_str() {
                            "v4" => ip.is_ipv4(),
                            "v6" => ip.is_ipv6(),
                            _ => true,
                        })
                        .collect();
                    println!("got the offer; listening on {addresses:?}");
                    open_on("sender", "\"Sender\"", msg["sdp"].as_str().unwrap(), addresses, signals.clone(), sink.clone()).unwrap();
                }
                Some("ice") => {
                    let _ = remote_candidate("sender", msg["candidate"].as_str().unwrap_or_default().to_string());
                }
                _ => {}
            }
        }
        // o que a recepção quer dizer a quem transmite
        while let Ok(msg) = from_watcher.try_recv() {
            match msg {
                Outgoing::Answer { sdp } => drop(post("answer", serde_json::json!({ "sdp": sdp })).await),
                Outgoing::Ice { candidate, .. } => drop(post("ice", serde_json::json!({ "candidate": candidate })).await),
                Outgoing::State { state } => states.push(state),
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    close("sender");

    let frames = sink.frames.lock().unwrap();
    let bytes: usize = frames.iter().map(|f| f.1).sum();
    let span = frames.last().map_or(Duration::ZERO, |f| f.2);
    println!(
        "{} frames ({} keyframes, {} KB) over {:.1}s of media = {:.1} fps, {:.0} kbps; {} audio packets; states {states:?}",
        frames.len(),
        frames.iter().filter(|f| f.0).count(),
        bytes / 1024,
        span.as_secs_f64(),
        frames.len() as f64 / span.as_secs_f64().max(0.001),
        bytes as f64 * 8.0 / 1000.0 / span.as_secs_f64().max(0.001),
        sink.audio.lock().unwrap(),
    );
    if let Ok(path) = std::env::var("WATCH_DUMP") {
        std::fs::write(&path, &*sink.dump.lock().unwrap()).unwrap();
        println!("saved to {path}");
    }
    assert!(frames.len() > 10, "video arrived");
    assert!(frames[0].0, "starts with a keyframe");
    assert!(frames.windows(2).all(|pair| pair[0].2 <= pair[1].2), "in order");
}
