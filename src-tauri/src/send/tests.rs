//! Teste manual do envio nativo com um navegador de verdade assistindo.
//!
//! `cargo test native_send_browser -- --ignored --nocapture` e abrir
//! http://127.0.0.1:3011 (só este PC) em uma ou mais abas (Chromium/Edge, o mesmo motor do
//! WebView2). A página faz o papel de quem assiste no app: recebe a oferta,
//! responde e troca rotas aos poucos (trickle), como `mesh.ts`. A sinalização
//! aqui é HTTP em vez da sala. NATIVE_SECS (padrão 120) define a duração.

use super::{Outgoing, Signals};
use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct HttpSignals {
    /// Mensagens esperando cada espectador buscar.
    queues: Mutex<HashMap<String, Vec<serde_json::Value>>>,
}

impl Signals for HttpSignals {
    fn send(&self, viewer: &str, msg: Outgoing) {
        let json = serde_json::to_value(&msg).unwrap();
        println!("-> {viewer}: {}", json.get("kind").and_then(|k| k.as_str()).unwrap_or("?"));
        self.queues.lock().unwrap().entry(viewer.to_string()).or_default().push(json);
    }
}

const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>Native send test</title>
<style>body{margin:0;background:#111;color:#ddd;font:13px system-ui}video{width:100%;max-height:75vh;background:#000}pre{margin:8px}</style>
<video id="v" autoplay muted playsinline></video><pre id="s">connecting...</pre>
<script>
const id = "viewer-" + Math.random().toString(36).slice(2, 7);
const pc = new RTCPeerConnection({ iceServers: [{ urls: ["stun:stun.cloudflare.com:3478"] }] });
const post = (path, body) => fetch(`${path}?id=${id}`, { method: "POST", body: JSON.stringify(body ?? {}) });
pc.ontrack = (e) => (document.getElementById("v").srcObject = e.streams[0] ?? new MediaStream([e.track]));
pc.onicecandidate = (e) => e.candidate && post("/ice", e.candidate.toJSON());
let pending = [];
async function poll() {
  const msgs = await (await fetch(`/events?id=${id}`)).json();
  for (const m of msgs) {
    if (m.kind === "offer") {
      await pc.setRemoteDescription({ type: "offer", sdp: m.sdp });
      const answer = await pc.createAnswer();
      await pc.setLocalDescription(answer);
      await post("/answer", { sdp: pc.localDescription.sdp });
      for (const c of pending) await pc.addIceCandidate(c);
      pending = [];
    } else if (m.kind === "ice") {
      const c = { candidate: m.candidate, sdpMid: m.mid, sdpMLineIndex: 0 };
      if (pc.remoteDescription) await pc.addIceCandidate(c); else pending.push(c);
    }
    window.log = (window.log ?? []).concat(m.kind + (m.state ? ":" + m.state : ""));
  }
  setTimeout(poll, 200);
}
let last = null;
setInterval(async () => {
  const r = await pc.getStats();
  let text = `connection ${pc.connectionState}, ICE ${pc.iceConnectionState}, events ${JSON.stringify(window.log ?? [])}`;
  r.forEach((s) => {
    if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") {
      const l = r.get(s.localCandidateId), rm = r.get(s.remoteCandidateId);
      window.route = `${l?.candidateType} ${l?.address}:${l?.port} -> ${rm?.candidateType} ${rm?.address}:${rm?.port}`;
    }
    if (s.type !== "inbound-rtp" || s.kind !== "video") return;
    const kbps = last ? Math.round(((s.bytesReceived - last.b) * 8) / (s.timestamp - last.t)) : 0;
    last = { b: s.bytesReceived, t: s.timestamp };
    window.lastVideo = { codec: r.get(s.codecId)?.sdpFmtpLine, w: s.frameWidth, h: s.frameHeight,
      fps: s.framesPerSecond, kbps, decoded: s.framesDecoded, lost: s.packetsLost, freezes: s.freezeCount,
      keyframes: s.keyFramesDecoded, plis: s.pliCount, jitterMs: Math.round(s.jitter * 1000), route: window.route };
    text += "\n" + JSON.stringify(window.lastVideo, null, 1);
  });
  document.getElementById("s").textContent = text;
}, 1000);
post("/watch").then(poll);
</script>"#;

#[test]
#[ignore]
fn native_send_browser() {
    let secs: u64 = std::env::var("NATIVE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let sink = super::start(60, 8_000_000);
    let (stop, video) = crate::capture::start_video_for_test(sink, 1080, 60);
    let signals = Arc::new(HttpSignals::default());

    let server = tiny_http::Server::http("127.0.0.1:3011").unwrap();
    println!("open http://127.0.0.1:3011 (only this PC; running for {secs}s)");
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let Ok(Some(mut req)) = server.recv_timeout(Duration::from_millis(200)) else { continue };
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let viewer = query.trim_start_matches("id=").to_string();
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let reply = match path {
            "/" => {
                let h = tiny_http::Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
                let _ = req.respond(tiny_http::Response::from_string(PAGE).with_header(h));
                continue;
            }
            "/watch" => format!("{:?}", super::open_viewer(&viewer, &format!("\"{viewer}\""), signals.clone())),
            "/answer" => format!("{:?}", super::answer(&viewer, json["sdp"].as_str().unwrap_or("").into())),
            "/ice" => format!("{:?}", super::remote_candidate(&viewer, json["candidate"].as_str().unwrap_or("").into())),
            "/events" => {
                let msgs = signals.queues.lock().unwrap().remove(&viewer).unwrap_or_default();
                serde_json::to_string(&msgs).unwrap()
            }
            _ => "not found".into(),
        };
        let _ = req.respond(tiny_http::Response::from_string(reply));
    }
    stop.store(true, Ordering::Relaxed);
    (video.stop)();
    super::stop();
    std::thread::sleep(Duration::from_millis(300));
}
