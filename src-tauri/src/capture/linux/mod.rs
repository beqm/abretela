//! Captura no Linux: telas e janelas pelo portal do sistema
//! (xdg-desktop-portal) com o PipeWire, e o som do sistema pelo PipeWire.
//!
//! No Wayland um app não enxerga as telas nem as janelas dos outros: quem
//! escolhe o que transmitir é a pessoa, numa janela do próprio sistema. Por
//! isso aqui não há lista nem miniaturas: existe uma fonte só ([`PORTAL`]) e,
//! ao começar, o sistema pergunta. O que o sistema entrega (BGRx, na memória)
//! é reduzido e convertido para I420 no processador, como na reserva do
//! Windows.
//!
//! São três threads por captura: uma conversa com o portal (D-Bus) e mantém a
//! sessão aberta; outra roda o laço do PipeWire, que entrega os frames; a
//! terceira reduz e converte. A conversão fica fora do laço do PipeWire
//! porque ele não pode demorar: enquanto um retorno trabalha, os frames
//! seguintes se acumulam e nada mais anda, nem o pedido para parar.

mod audio;
mod frame;

pub use audio::PipewireAudio;

use super::{
    contracts::{AudioTarget, FrameSink, RunningVideo, Source, SourceInfo, VideoConfig, VideoSource},
    convert::{bgra_to_i420, BgraScaler},
    packet::{PixelFormat, VideoPacket},
};
use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
    PersistMode, ResponseError,
};
use frame::{pack_part, visible_part, Layout, Pacer, Part};
use futures_util::StreamExt;
use pipewire::{self as pw, properties::properties, spa};
use spa::{
    buffer::meta::MetaVideoCrop,
    param::video::{VideoFormat, VideoInfoRaw},
};
use std::{
    os::fd::OwnedFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// A única fonte do Linux: "o que a pessoa escolher na janela do sistema".
pub const PORTAL: &str = "portal";
/// O erro de `start` quando a pessoa fecha a janela do sistema sem escolher.
pub const CANCELLED: &str = "cancelled";
/// De quanto em quanto tempo as threads olham se é para parar.
const STOP_CHECK: Duration = Duration::from_millis(200);

/// Baixa a prioridade da thread atual, para a captura não disputar o
/// processador com o que está sendo transmitido.
pub fn lower_thread_priority() {
    // no Linux a prioridade (nice) vale por thread: 0 = a thread que chama
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// O que a pessoa escolheu, pronto para o PipeWire.
struct Chosen {
    /// O nó do PipeWire com as imagens.
    node: u32,
    /// A conexão com o PipeWire que o portal abriu para este app. Sem ela
    /// (só nos testes), vale o PipeWire da sessão.
    remote: Option<OwnedFd>,
    /// Para o log.
    what: String,
}

fn describe(e: ashpd::Error) -> String {
    match e {
        ashpd::Error::Response(ResponseError::Cancelled) => CANCELLED.into(),
        ashpd::Error::PortalNotFound(_) => {
            "this system has no screen sharing portal (xdg-desktop-portal with a ScreenCast backend)".into()
        }
        other => format!("the system's screen sharing portal failed: {other}"),
    }
}

/// Pede ao sistema a tela ou a janela, avisa por `ready` e segura a sessão
/// aberta até `stop` (ou até a pessoa encerrar pelo próprio sistema, e aí é
/// esta função que liga o `stop`).
async fn portal_session(stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<Chosen, String>>) {
    let opened = async {
        let proxy = Screencast::new().await?;
        let session = proxy.create_session(Default::default()).await?;
        let chosen = async {
            // o cursor desenhado na imagem, quando o sistema sabe fazer isso
            let cursor = match proxy.available_cursor_modes().await {
                Ok(modes) if modes.contains(CursorMode::EMBEDDED) => CursorMode::EMBEDDED,
                _ => CursorMode::HIDDEN,
            };
            proxy
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_cursor_mode(cursor)
                        .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                        .set_multiple(false)
                        .set_restore_token(None)
                        .set_persist_mode(PersistMode::DoNot),
                )
                .await?
                .response()?;
            let streams = proxy.start(&session, None, Default::default()).await?.response()?;
            let stream = streams.streams().first().ok_or(ashpd::Error::NoResponse)?.to_owned();
            let remote = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
            let kind = match stream.source_type() {
                Some(SourceType::MONITOR) => "screen",
                Some(SourceType::WINDOW) => "window",
                _ => "source",
            };
            let size = stream.size().map_or(String::new(), |(w, h)| format!(", {w}x{h}"));
            let what = format!("a {kind} chosen in the system's dialog{size}");
            Ok::<_, ashpd::Error>(Chosen { node: stream.pipe_wire_node_id(), remote: Some(remote), what })
        };
        match chosen.await {
            Ok(chosen) => Ok((session, chosen)),
            Err(e) => {
                // desistir na janela do sistema não fecha a sessão; a conexão
                // com o D-Bus dura o app todo, então ela ficaria aberta até lá
                let _ = session.close().await;
                Err(e)
            }
        }
    };
    let session = match opened.await {
        Ok((session, chosen)) => {
            if ready.send(Ok(chosen)).is_err() {
                // quem pediu já desistiu
                let _ = session.close().await;
                return;
            }
            session
        }
        Err(e) => {
            let _ = ready.send(Err(describe(e)));
            return;
        }
    };

    // "parar de compartilhar" no próprio sistema fecha a sessão por lá, e
    // fechar a janela transmitida também
    let closed = session.receive_closed().await;
    let ended_by_system = async {
        match closed {
            Ok(closed) => {
                std::pin::pin!(closed).next().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    let asked_to_stop = async {
        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(STOP_CHECK).await;
        }
    };
    tokio::select! {
        _ = ended_by_system => {
            crate::log_info!("capture", "The system ended the screen sharing (stopped from its own indicator, or what was shared was closed)");
            stop.store(true, Ordering::Relaxed);
        }
        _ = asked_to_stop => {
            let _ = session.close().await;
        }
    }
}

/// Onde rodam as conversas com o portal: um runtime só, que dura o app todo.
///
/// O `ashpd` abre a conexão com o D-Bus na primeira conversa e a reaproveita
/// nas seguintes, e quem lê essa conexão são tarefas do runtime em que ela
/// nasceu. Com um runtime por captura, essas tarefas morriam junto com a
/// primeira captura e a segunda esperava para sempre uma resposta que
/// ninguém lia: o sistema nem chegava a perguntar de novo.
fn portal_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("capture-portal-io")
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Roda uma conversa com o portal até o fim. Bloqueia: roda na própria thread.
fn talk_to_portal(stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<Chosen, String>>) {
    match portal_runtime() {
        Ok(runtime) => runtime.block_on(portal_session(stop, ready)),
        Err(e) => {
            let _ = ready.send(Err(format!("could not start the portal thread: {e}")));
        }
    }
}

/// Um frame copiado do sistema: BGRA, sem o enchimento das linhas.
struct Raw {
    bgra: Vec<u8>,
    w: u32,
    h: u32,
}

/// Por onde os frames passam do laço do PipeWire para a conversão. Cabe um
/// só: se a conversão ainda não pegou o anterior, o novo fica no lugar dele.
#[derive(Default)]
struct Mailbox {
    slot: Mutex<Slot>,
    arrived: Condvar,
}

#[derive(Default)]
struct Slot {
    waiting: Option<Raw>,
    /// Buffers já usados, para não alocar um por frame.
    spare: Vec<Vec<u8>>,
}

impl Mailbox {
    /// Um buffer para copiar o próximo frame.
    fn buffer(&self) -> Vec<u8> {
        self.slot.lock().unwrap().spare.pop().unwrap_or_default()
    }

    fn put(&self, raw: Raw) {
        let mut slot = self.slot.lock().unwrap();
        if let Some(old) = slot.waiting.replace(raw) {
            slot.spare.push(old.bgra);
        }
        self.arrived.notify_one();
    }

    /// O frame que está esperando; `None` se nenhum chegou em `wait`.
    fn take(&self, wait: Duration) -> Option<Raw> {
        let slot = self.slot.lock().unwrap();
        let (mut slot, _) = self.arrived.wait_timeout_while(slot, wait, |s| s.waiting.is_none()).unwrap();
        slot.waiting.take()
    }

    fn give_back(&self, buffer: Vec<u8>) {
        let mut slot = self.slot.lock().unwrap();
        if slot.spare.len() < 3 {
            slot.spare.push(buffer);
        }
    }
}

/// Reduz e converte os frames que chegam por `mailbox` e os entrega, até `stop`.
fn convert_frames(mailbox: &Mailbox, config: VideoConfig, sink: &dyn FrameSink, stop: &AtomicBool) {
    let mut scaler = BgraScaler::default();
    let mut sent = 0u64;
    let max_h = config.max_height;
    let max_w = max_h * 21 / 9; // limita ultrawide
    while !stop.load(Ordering::Relaxed) {
        let Some(raw) = mailbox.take(STOP_CHECK) else { continue };
        match scaler.scale(&raw.bgra, raw.w, raw.h, max_w, max_h) {
            Ok((dw, dh, scaled)) => {
                let mut packet = VideoPacket::new(PixelFormat::I420, dw, dh);
                bgra_to_i420(dw as usize, dh as usize, &scaled, packet.pixels_mut());
                sent += 1;
                if sent == 1 {
                    crate::log_info!(
                        "capture",
                        "First frame captured: {}x{} from the system, sent as {dw}x{dh} (converted on the {})",
                        raw.w,
                        raw.h,
                        packet.format().converted_on()
                    );
                }
                sink.video(packet);
            }
            Err(e) if sent == 0 => crate::log_warn!("capture", "Could not resize a frame: {e}"),
            Err(_) => {}
        }
        mailbox.give_back(raw.bgra);
    }
}

/// O que os retornos do PipeWire dividem.
struct Frames {
    mailbox: Arc<Mailbox>,
    stop: Arc<AtomicBool>,
    /// Para sair do laço de dentro de um retorno.
    mainloop: pw::main_loop::MainLoopWeak,
    info: VideoInfoRaw,
    layout: Option<Layout>,
    pacer: Pacer,
    /// Para avisar só uma vez de frames que não dá para ler.
    warned: bool,
    /// O pedaço dos frames que está sendo enviado, para o log dizer quando muda.
    part: Option<Part>,
}

impl Frames {
    fn warn_once(&mut self, why: &str) {
        if !self.warned {
            self.warned = true;
            crate::log_warn!("capture", "Skipping frames from the system: {why}");
        }
    }

    /// O formato que o sistema escolheu entre os que o app aceita.
    fn format_changed(&mut self, param: &spa::pod::Pod) {
        use spa::param::format::{MediaSubtype, MediaType};
        let Ok((media, subtype)) = spa::param::format_utils::parse_format(param) else { return };
        if media != MediaType::Video || subtype != MediaSubtype::Raw || self.info.parse(param).is_err() {
            return;
        }
        let format = self.info.format();
        self.layout = match format {
            VideoFormat::BGRx | VideoFormat::BGRA => Some(Layout::Bgrx),
            VideoFormat::RGBx | VideoFormat::RGBA => Some(Layout::Rgbx),
            _ => None,
        };
        let size = self.info.size();
        crate::log_info!(
            "capture",
            "The system is sending {}x{} frames as {format:?}{}",
            size.width,
            size.height,
            if self.layout.is_none() { " (a format this app cannot read)" } else { "" }
        );
    }

    /// Um frame chegou: copia sem o enchimento e passa para a conversão.
    /// Roda no laço do PipeWire, então só faz o que é rápido.
    fn frame(&mut self, stream: &pw::stream::Stream) {
        if self.stop.load(Ordering::Relaxed) {
            // com frames se acumulando, o relógio do laço pode nem ser chamado
            if let Some(mainloop) = self.mainloop.upgrade() {
                mainloop.quit();
            }
            return;
        }
        let Some(mut buffer) = stream.dequeue_buffer() else { return };
        // se há mais de um frame esperando, só o mais novo interessa (os
        // outros voltam para o sistema ao sair daqui)
        while let Some(newer) = stream.dequeue_buffer() {
            buffer = newer;
        }
        // de uma janela, o sistema manda o frame do tamanho da tela e avisa
        // onde ela está nele
        let crop = buffer.find_meta::<MetaVideoCrop>().map(MetaVideoCrop::meta_region).filter(|r| r.is_valid()).map(|r| {
            let (at, size) = (r.position(), r.size());
            (at.x, at.y, size.width, size.height)
        });
        let Some(data) = buffer.datas_mut().first_mut() else { return };
        let (size, stride, offset) = {
            let chunk = data.chunk();
            (chunk.size() as usize, chunk.stride(), chunk.offset() as usize)
        };
        // sem imagem nova (ex.: só o cursor mexeu, em outro modo de cursor)
        if size == 0 {
            return;
        }
        let Some(layout) = self.layout else {
            return self.warn_once("the format is not one this app reads");
        };
        if !self.pacer.due(Instant::now()) {
            return;
        }
        let (w, h) = (self.info.size().width as usize, self.info.size().height as usize);
        let stride = if stride > 0 { stride as usize } else { w * 4 };
        let Some(bytes) = data.data().and_then(|d| d.get(offset..)) else {
            return self.warn_once("the frame is not in memory this app can read");
        };
        let part = visible_part(w, h, crop);
        if self.part != Some(part) {
            // de uma janela que muda de tamanho isto muda a cada frame: só o
            // primeiro recorte de cada captura vai para o log
            if self.part.is_none() && (part.w, part.h) != (w, h) {
                crate::log_info!(
                    "capture",
                    "The picture is a {}x{} part of each {w}x{h} frame (a window): sending only that part",
                    part.w,
                    part.h
                );
            }
            self.part = Some(part);
        }
        let mut bgra = self.mailbox.buffer();
        if pack_part(bytes, stride, part, layout, &mut bgra) {
            self.mailbox.put(Raw { bgra, w: part.w as u32, h: part.h as u32 });
        } else {
            self.mailbox.give_back(bgra);
            self.warn_once("a frame came smaller than its size says");
        }
    }
}

/// Os formatos que o app aceita, para o PipeWire combinar com o sistema.
fn accepted_formats(fps: u32) -> Result<Vec<u8>, String> {
    use spa::{
        param::{
            format::{FormatProperties, MediaSubtype, MediaType},
            ParamType,
        },
        pod::{object, property, serialize::PodSerializer, Value},
        utils::{Fraction, Rectangle, SpaTypes},
    };
    let formats = object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        // só formatos de 4 bytes por pixel na memória; o primeiro é o preferido
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        // qualquer taxa: alguns sistemas só oferecem a do monitor, e o ritmo
        // escolhido é garantido aqui mesmo (`Pacer`)
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: fps, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(formats))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("could not describe the video formats: {e:?}"))
}

/// O pedido do aviso de recorte (`SPA_META_VideoCrop`) em cada frame.
fn wanted_metadata() -> Result<Vec<u8>, String> {
    use spa::{
        param::ParamType,
        pod::{serialize::PodSerializer, Object, Property, PropertyFlags, Value},
        utils::{Id, SpaTypes},
    };
    let meta = |key: u32, value: Value| Property { key, flags: PropertyFlags::empty(), value };
    let wanted = Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            meta(spa::sys::SPA_PARAM_META_type, Value::Id(Id(spa::sys::SPA_META_VideoCrop))),
            meta(spa::sys::SPA_PARAM_META_size, Value::Int(std::mem::size_of::<spa::sys::spa_meta_region>() as i32)),
        ],
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(wanted))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("could not describe the frame metadata: {e:?}"))
}

/// Recebe os frames do nó escolhido até `stop`. Bloqueia: roda na própria thread.
fn receive_frames(chosen: Chosen, config: VideoConfig, sink: Arc<dyn FrameSink>, stop: Arc<AtomicBool>) -> Result<(), String> {
    let failed = |what: &'static str| move |e: pw::Error| format!("PipeWire: {what}: {e}");
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(failed("could not start"))?;
    let context = pw::context::ContextBox::new(mainloop.loop_(), None).map_err(failed("could not start"))?;
    let core = match chosen.remote {
        Some(remote) => context.connect_fd(remote, None),
        None => context.connect(None),
    }
    .map_err(failed("could not connect"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "abretela-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(failed("could not create the stream"))?;

    let mailbox = Arc::new(Mailbox::default());
    let frames = Frames {
        mailbox: mailbox.clone(),
        stop: stop.clone(),
        mainloop: mainloop.downgrade(),
        info: VideoInfoRaw::default(),
        layout: None,
        pacer: Pacer::new(config.fps),
        warned: false,
        part: None,
    };
    let _listener = stream
        .add_local_listener_with_user_data(frames)
        .state_changed(|_, frames, old, new| {
            use pw::stream::StreamState;
            match new {
                StreamState::Error(e) => {
                    crate::log_error!("capture", "The capture stream failed: {e}");
                    frames.stop.store(true, Ordering::Relaxed);
                }
                // o que era transmitido sumiu (a janela fechou, a tela foi desligada)
                StreamState::Unconnected if matches!(old, StreamState::Streaming | StreamState::Paused) => {
                    crate::log_info!("capture", "What was being captured is gone; stopping");
                    frames.stop.store(true, Ordering::Relaxed);
                }
                _ => {}
            }
        })
        .param_changed(|stream, frames, id, param| {
            if let (Some(param), true) = (param, id == spa::param::ParamType::Format.as_raw()) {
                frames.format_changed(param);
                // o sistema só avisa onde a janela está no frame para quem pede
                match wanted_metadata() {
                    Ok(wanted) => {
                        if let Some(pod) = spa::pod::Pod::from_bytes(&wanted) {
                            if let Err(e) = stream.update_params(&mut [pod]) {
                                crate::log_warn!("capture", "Could not ask the system where a window is in its frames: {e}");
                            }
                        }
                    }
                    Err(e) => crate::log_warn!("capture", "{e}"),
                }
            }
        })
        .process(|stream, frames| frames.frame(stream))
        .register()
        .map_err(failed("could not listen to the stream"))?;

    let formats = accepted_formats(config.fps)?;
    let mut params = [spa::pod::Pod::from_bytes(&formats).ok_or("PipeWire: the video formats came out invalid")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(chosen.node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(failed("could not connect the stream"))?;

    let converter = {
        let (sink, stop) = (sink.clone(), stop.clone());
        spawn("capture-convert", move || convert_frames(&mailbox, config, sink.as_ref(), &stop))?
    };

    let ran = run_until_stopped(&mainloop, &stop, &sink);
    // a conversão para junto com os frames
    stop.store(true, Ordering::Relaxed);
    let _ = converter.join();
    ran
}

/// Roda o laço do PipeWire até `stop` (ou até o destino fechar, e aí liga o
/// `stop`). O laço só volta quando alguém manda: um relógio olha o `stop`.
fn run_until_stopped(mainloop: &pw::main_loop::MainLoopRc, stop: &Arc<AtomicBool>, sink: &Arc<dyn FrameSink>) -> Result<(), String> {
    let weak = mainloop.downgrade();
    let (stop, sink) = (stop.clone(), sink.clone());
    let timer = mainloop.loop_().add_timer(move |_| {
        if sink.is_closed() {
            stop.store(true, Ordering::Relaxed);
        }
        if stop.load(Ordering::Relaxed) {
            if let Some(mainloop) = weak.upgrade() {
                mainloop.quit();
            }
        }
    });
    timer
        .update_timer(Some(STOP_CHECK), Some(STOP_CHECK))
        .into_result()
        .map_err(|e| format!("PipeWire: could not arm the stop timer: {e}"))?;
    mainloop.run();
    Ok(())
}

pub struct PortalSource;

impl VideoSource for PortalSource {
    fn list(&self) -> Vec<Source> {
        vec![Source { id: PORTAL.into(), kind: "system", title: "Screen or window".into(), app: String::new() }]
    }

    fn thumbnail(&self, _id: &str) -> Option<String> {
        None
    }

    fn inspect(&self, id: &str) -> Option<SourceInfo> {
        // o som é sempre o do sistema inteiro (`audio.rs`): não há processo a escolher
        (id == PORTAL).then(|| SourceInfo {
            description: "what gets chosen in the system's dialog".into(),
            audio: Some(AudioTarget { pid: 0, include_tree: false }),
        })
    }

    fn start(
        &self,
        id: &str,
        config: VideoConfig,
        sink: Arc<dyn FrameSink>,
        stop: Arc<AtomicBool>,
    ) -> Result<RunningVideo, String> {
        if id != PORTAL {
            return Err("fonte inválida".into());
        }
        // a conversa com o portal é assíncrona e dura a captura toda
        let (ready, chosen) = mpsc::channel();
        let portal_stop = stop.clone();
        let portal = spawn("capture-portal", move || talk_to_portal(portal_stop, ready))?;
        // espera a pessoa escolher (ou desistir) na janela do sistema
        let chosen = match chosen.recv() {
            Ok(Ok(chosen)) => chosen,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("the portal thread stopped before answering".into()),
        };
        crate::log_info!("capture", "Capturing {}", chosen.what);

        let frames_stop = stop.clone();
        let frames = spawn("capture-pipewire", move || {
            if let Err(e) = receive_frames(chosen, config, sink, frames_stop.clone()) {
                crate::log_error!("capture", "The capture stopped: {e}");
            }
            // sem frames não há captura: a sessão do portal fecha junto
            frames_stop.store(true, Ordering::Relaxed);
        });
        let frames = match frames {
            Ok(frames) => frames,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                return Err(e);
            }
        };
        Ok(RunningVideo {
            stop: Box::new(move || {
                stop.store(true, Ordering::Relaxed);
                let _ = frames.join();
                let _ = portal.join();
            }),
        })
    }
}

fn spawn(name: &str, run: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, String> {
    thread::Builder::new().name(name.into()).spawn(run).map_err(|e| format!("could not start the {name} thread: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use std::sync::Mutex;

    /// Guarda cada frame e quando chegou.
    struct Collect(Mutex<Vec<(Instant, VideoPacket)>>);

    impl FrameSink for Collect {
        fn video(&self, packet: VideoPacket) {
            self.0.lock().unwrap().push((Instant::now(), packet));
        }
        fn audio(&self, _: Bytes) {}
        fn is_closed(&self) -> bool {
            false
        }
    }

    /// O que uma conversa com o portal deixa rodando (a leitura da conexão
    /// com o D-Bus) continua rodando depois que a thread dela acaba.
    #[test]
    fn what_a_portal_conversation_spawns_outlives_it() {
        let (ticks, ticked) = mpsc::channel();
        thread::spawn(move || {
            portal_runtime().unwrap().block_on(async move {
                tokio::spawn(async move {
                    while ticks.send(()).is_ok() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                });
            })
        })
        .join()
        .unwrap();
        // a conversa acabou; o que chegar daqui em diante veio depois dela
        while ticked.try_recv().is_ok() {}
        assert!(ticked.recv_timeout(Duration::from_secs(5)).is_ok(), "the task died with the conversation");
    }

    /// Duas conversas seguidas com o portal, cada uma na sua thread, como em
    /// duas capturas seguidas. A segunda tem de ter resposta como a primeira
    /// (a conexão com o D-Bus é a mesma nas duas). Precisa de um D-Bus de
    /// sessão; sem portal instalado a resposta é o erro de que ele não existe:
    ///
    /// ```sh
    /// dbus-run-session -- cargo test portal_answers_twice -- --ignored --nocapture
    /// ```
    ///
    /// Num desktop de verdade o sistema pergunta duas vezes o que compartilhar.
    #[test]
    #[ignore]
    fn portal_answers_twice() {
        for round in 1..=2 {
            let (ready, answer) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let talk = thread::spawn(move || talk_to_portal(flag, ready));
            let answer = answer.recv_timeout(Duration::from_secs(60));
            println!("round {round}: {:?}", answer.as_ref().map(|a| a.as_ref().map(|c| c.what.clone())));
            assert!(answer.is_ok(), "round {round}: the portal conversation never answered");
            stop.store(true, Ordering::Relaxed);
            talk.join().unwrap();
        }
    }

    /// Recebe frames de um nó de vídeo do PipeWire da sessão, sem o portal
    /// (que precisa de alguém para escolher a tela). Com um nó de teste:
    ///
    /// ```sh
    /// gst-launch-1.0 videotestsrc pattern=red ! video/x-raw,format=BGRx,width=1282,height=722,framerate=60/1 ! pipewiresink
    /// PW_NODE=<id que o `pw-cli ls Node` mostra> cargo test --release pipewire_frames -- --ignored --nocapture
    /// ```
    ///
    /// `PW_EXPECT=63,102,240` confere a cor do meio (Y,U,V; estes são os do vermelho).
    #[test]
    #[ignore]
    fn pipewire_frames() {
        let node = std::env::var("PW_NODE").expect("PW_NODE").parse().expect("a node id");
        let sink = Arc::new(Collect(Mutex::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let chosen = Chosen { node, remote: None, what: "a test node".into() };
        let config = VideoConfig { max_height: 720, fps: 30, gpu: false };
        let (frames, flag) = (sink.clone(), stop.clone());
        let capture = thread::spawn(move || receive_frames(chosen, config, frames, flag));
        thread::sleep(Duration::from_secs(3));
        stop.store(true, Ordering::Relaxed);
        capture.join().unwrap().expect("the capture ran");

        let frames = sink.0.lock().unwrap();
        let (started, first) = frames.first().expect("at least one frame");
        // a taxa conta do primeiro frame em diante: conectar leva um tempo
        let fps = (frames.len() - 1) as f64 / frames.last().unwrap().0.duration_since(*started).as_secs_f64();
        let (w, h) = (first.width() as usize, first.height() as usize);
        let px = first.pixels();
        // o meio da imagem: Y, U e V
        let (y, u, v) = (px[w * (h / 2) + w / 2], px[w * h + (w / 2) * (h / 4) + w / 4], px[w * h * 5 / 4 + (w / 2) * (h / 4) + w / 4]);
        println!("{} frames at {fps:.1} fps, {w}x{h} {:?}, the middle pixel is Y={y} U={u} V={v}", frames.len(), first.format());
        if let Ok(expected) = std::env::var("PW_EXPECT") {
            let expected: Vec<i32> = expected.split(',').map(|n| n.parse().unwrap()).collect();
            for (got, want) in [y, u, v].into_iter().zip(expected) {
                assert!((got as i32 - want).abs() <= 2, "expected Y,U,V near {:?}, got {y},{u},{v}", std::env::var("PW_EXPECT"));
            }
        }
        // sem otimização a conversão não acompanha: aí só vale o teto
        let floor = if cfg!(debug_assertions) { 1.0 } else { 28.0 };
        assert!((floor..=32.0).contains(&fps), "30 fps, whatever the source sends: {fps:.1}");
        assert!(h <= 720 && w % 2 == 0 && h % 2 == 0, "reduced to fit, with even sizes: {w}x{h}");
    }
}
