//! Captura nativa de telas, janelas e som. Junta as peças de `contracts.rs`:
//! a fonte de vídeo e de áudio do sistema (hoje `win/`), a conversão e o
//! destino dos pacotes (hoje o WebSocket local de `sink.rs`, que o app
//! transforma em faixas para o WebRTC).

pub mod commands;
pub mod contracts;
mod convert;
pub mod packet;
mod sink;
mod win;

use contracts::{AudioSource, FrameSink, VideoConfig, VideoSource};
pub use contracts::Source;
use sink::WebSocketSink;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};

/// Peças específicas do sistema (prioridade de thread etc.).
use win as platform;

static VIDEO: win::WgcSource = win::WgcSource;
static AUDIO: win::WasapiLoopback = win::WasapiLoopback;

pub use sink::serve;

struct Session {
    token: String,
    stop: Arc<AtomicBool>,
    stop_video: Option<Box<dyn FnOnce() + Send>>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn random_token() -> String {
    use std::hash::{BuildHasher, RandomState};
    let a = RandomState::new().hash_one(Instant::now());
    let b = RandomState::new().hash_one(std::process::id());
    format!("{a:016x}{b:016x}")
}

pub fn list_sources() -> Vec<Source> {
    VIDEO.list()
}

pub fn thumbnail(id: &str) -> Option<String> {
    VIDEO.thumbnail(id)
}

pub fn stop() {
    stop_if(None);
}

/// Começa a capturar `id`; devolve a URL do WebSocket local com os frames.
pub fn start(id: &str, height: u32, fps: u32, audio: bool, gpu: bool) -> Result<String, String> {
    stop();
    let token = random_token();
    let stop = Arc::new(AtomicBool::new(false));
    let fps = fps.clamp(5, 60);
    let (sink, url) = WebSocketSink::open(&token, fps, stop.clone())?;
    let sink: Arc<dyn FrameSink> = sink;
    let Some(info) = VIDEO.inspect(id) else {
        stop.store(true, Ordering::Relaxed);
        WebSocketSink::discard(&token);
        return Err("fonte inválida".into());
    };
    log_info!(
        "capture",
        "Starting capture of {} at up to {height}p {fps}fps, audio {}, hardware acceleration {}",
        info.description,
        if audio { "on" } else { "off" },
        if gpu { "on" } else { "off" }
    );

    let config = VideoConfig { max_height: height, fps, gpu };
    let video = match VIDEO.start(id, config, sink.clone(), stop.clone()) {
        Ok(video) => video,
        Err(e) => {
            log_error!("capture", "Windows refused to start the capture: {e}");
            stop.store(true, Ordering::Relaxed);
            WebSocketSink::discard(&token);
            return Err(e);
        }
    };

    if let (true, Some(target)) = (audio, info.audio) {
        let stop = stop.clone();
        thread::spawn(move || {
            if let Err(e) = AUDIO.run(target, sink, stop) {
                log_error!("audio", "Audio capture failed: {e}");
            }
        });
    }

    *SESSION.lock().unwrap() = Some(Session { token, stop, stop_video: Some(video.stop) });
    Ok(url)
}

/// Para a sessão atual; com `token`, só se for aquela sessão.
pub fn stop_if(token: Option<&str>) {
    let session = {
        let mut current = SESSION.lock().unwrap();
        match (current.as_ref(), token) {
            (Some(s), Some(t)) if s.token != t => None,
            _ => current.take(),
        }
    };
    if let Some(mut s) = session {
        log_info!("capture", "Capture stopped");
        s.stop.store(true, Ordering::Relaxed);
        WebSocketSink::discard(&s.token);
        if let Some(stop_video) = s.stop_video.take() {
            stop_video();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::packet::{PixelFormat, HEADER};
    use super::*;
    use std::time::Duration;

    /// `cargo test capture_smoke -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn capture_smoke() {
        tokio::spawn(serve());
        tokio::time::sleep(Duration::from_millis(200)).await;

        let sources = list_sources();
        for s in &sources {
            println!("{} [{}] {} — {}", s.id, s.kind, s.title, s.app);
        }
        let pick = std::env::var("CAPTURE_SOURCE").unwrap_or_else(|_| "screen".into());
        let screen = sources
            .iter()
            .find(|s| s.kind == pick || s.title.contains(&pick))
            .unwrap()
            .id
            .clone();
        let height: u32 = std::env::var("CAPTURE_H").ok().and_then(|v| v.parse().ok()).unwrap_or(720);
        let fps: u32 = std::env::var("CAPTURE_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
        let t = Instant::now();
        let thumb = thumbnail(&screen);
        println!("thumbnail: {:?} bytes em {:?}", thumb.map(|t| t.len()), t.elapsed());

        let gpu = std::env::var("CAPTURE_GPU").map_or(true, |v| v != "0");
        let url = start(&screen, height, fps, true, gpu).unwrap();
        println!("url: {url}");
        let token = url.rsplit('/').next().unwrap();
        let (mut video, mut audio) = sink::take_waiting(token).unwrap();
        let (mut nv, mut na, mut dims, mut fmt) = (0, 0, (0u16, 0u16), 0u8);
        let mut last = None;
        let secs: u64 = std::env::var("CAPTURE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            tokio::select! {
                Some(v) = video.recv() => { nv += 1; fmt = v[1]; dims = (u16::from_le_bytes([v[2], v[3]]), u16::from_le_bytes([v[4], v[5]])); last = Some(v); }
                Some(_) = audio.recv() => na += 1,
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        stop();
        println!(
            "video frames: {nv} ({}x{}, {}), audio packets: {na}",
            dims.0,
            dims.1,
            if fmt == PixelFormat::Nv12 as u8 { "NV12/GPU" } else { "I420/CPU" }
        );
        if let (Ok(path), Some(pkt)) = (std::env::var("CAPTURE_DUMP"), last) {
            dump_jpeg(&pkt, &path);
            println!("frame salvo em {path}");
        }
        assert!(nv > 0);
    }

    /// Converte um pacote de vídeo (I420 ou NV12, BT.709 limitado) de volta
    /// para RGB e salva em JPEG, para conferir as cores a olho.
    fn dump_jpeg(pkt: &[u8], path: &str) {
        let w = u16::from_le_bytes([pkt[2], pkt[3]]) as usize;
        let h = u16::from_le_bytes([pkt[4], pkt[5]]) as usize;
        let data = &pkt[HEADER..];
        let (y, uv) = data.split_at(w * h);
        let mut rgb = vec![0u8; w * h * 3];
        for row in 0..h {
            for col in 0..w {
                let ci = (row / 2) * (w / 2) + col / 2;
                let (u, v) = if pkt[1] == PixelFormat::Nv12 as u8 {
                    let i = (row / 2) * w + (col / 2) * 2;
                    (uv[i], uv[i + 1])
                } else {
                    (uv[ci], uv[w * h / 4 + ci])
                };
                let yf = (y[row * w + col] as f32 - 16.0) * 1.164;
                let (uf, vf) = (u as f32 - 128.0, v as f32 - 128.0);
                let px = &mut rgb[(row * w + col) * 3..][..3];
                px[0] = (yf + 1.793 * vf).clamp(0.0, 255.0) as u8;
                px[1] = (yf - 0.213 * uf - 0.533 * vf).clamp(0.0, 255.0) as u8;
                px[2] = (yf + 2.112 * uf).clamp(0.0, 255.0) as u8;
            }
        }
        jpeg_encoder::Encoder::new(std::fs::File::create(path).unwrap(), 85)
            .encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
            .unwrap();
    }
}
