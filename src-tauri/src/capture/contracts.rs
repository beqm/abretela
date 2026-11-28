//! Contratos da captura. Cada peça é trocável sem mexer no resto:
//!
//! - [`VideoSource`]: de onde vêm as imagens (hoje Windows Graphics Capture).
//! - [`AudioSource`]: de onde vem o som (hoje WASAPI process loopback).
//! - [`FrameConverter`]: como um frame vira um [`VideoPacket`] no tamanho
//!   certo (hoje D3D11 para NV12 na placa, ou I420 no processador).
//! - [`FrameSink`]: para onde vão os pacotes (hoje o WebSocket local que o
//!   app lê; no envio nativo, seria o codificador).
//!
//! Outro sistema (Mac, Linux) entra com a própria fonte de vídeo e de áudio e
//! o próprio conversor; o destino e o formato dos pacotes continuam iguais.

use super::packet::VideoPacket;
use axum::body::Bytes;
use serde::Serialize;
use std::sync::{atomic::AtomicBool, Arc};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Uma tela ou janela que dá para transmitir.
#[derive(Serialize)]
pub struct Source {
    pub id: String,
    /// "screen" ou "window"; "system" quando é o próprio sistema que pergunta
    /// a tela ou a janela ao começar (Linux), e então a lista só tem esta.
    pub kind: &'static str,
    pub title: String,
    pub app: String,
}

/// O que vale saber de uma fonte antes de começar.
pub struct SourceInfo {
    /// Para o log (ex.: `window "Jogo" (jogo.exe)`).
    pub description: String,
    /// De onde capturar o som que acompanha esta fonte.
    pub audio: Option<AudioTarget>,
}

/// De que processo capturar o som.
// no Linux o som é o do sistema inteiro, e ninguém lê os campos
#[cfg_attr(target_os = "linux", allow(dead_code))]
#[derive(Clone, Copy)]
pub struct AudioTarget {
    pub pid: u32,
    /// `true`: só o processo (e filhos); `false`: tudo menos ele.
    pub include_tree: bool,
}

/// Como capturar o vídeo.
#[derive(Clone, Copy)]
pub struct VideoConfig {
    /// Altura máxima do frame enviado (a largura vem da proporção).
    pub max_height: u32,
    pub fps: u32,
    /// Converter na placa de vídeo (aceleração de hardware ligada).
    // no Linux a conversão é sempre no processador
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub gpu: bool,
}

/// Captura de vídeo em andamento; parar = chamar `stop`.
pub struct RunningVideo {
    pub stop: Box<dyn FnOnce() + Send>,
}

pub trait VideoSource: Sync {
    /// Telas e janelas disponíveis agora.
    fn list(&self) -> Vec<Source>;
    /// Uma imagem pequena da fonte (data URL JPEG), ou `None`.
    fn thumbnail(&self, id: &str) -> Option<String>;
    /// Descreve a fonte; `None` se o id não vale.
    fn inspect(&self, id: &str) -> Option<SourceInfo>;
    /// Começa a mandar frames para `sink` até `stop` virar `true` (ou a
    /// fonte sumir, e aí a própria fonte liga o `stop`).
    fn start(
        &self,
        id: &str,
        config: VideoConfig,
        sink: Arc<dyn FrameSink>,
        stop: Arc<AtomicBool>,
    ) -> Result<RunningVideo, String>;
}

pub trait AudioSource: Sync {
    /// Captura o som de `target` e manda para `sink` até `stop` ou até o
    /// destino fechar. Bloqueia: roda numa thread própria.
    fn run(&self, target: AudioTarget, sink: Arc<dyn FrameSink>, stop: Arc<AtomicBool>) -> Result<(), BoxError>;

    /// Os apps com som aberto agora, que dá para mutar para quem assiste na
    /// tela inteira. Vazio quando o sistema não sabe separar o som por app.
    fn apps(&self) -> Vec<AudioApp> {
        Vec::new()
    }

    /// Como `run`, na tela inteira, mas sem o som dos apps de `muted` (os
    /// `AudioApp::id`). Onde o sistema não sabe separar, é igual a `run`.
    fn run_muting(
        &self,
        target: AudioTarget,
        muted: &[String],
        sink: Arc<dyn FrameSink>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), BoxError> {
        let _ = muted;
        self.run(target, sink, stop)
    }
}

/// Um app com som aberto, para a lista de mutar.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AudioApp {
    /// O que identifica o app entre uma live e outra (o nome do executável,
    /// em minúsculas e sem `.exe`): é o que fica salvo como mutado.
    pub id: String,
    /// O nome para mostrar.
    pub name: String,
}

/// Converte frames do tipo `F` (o frame da fonte de vídeo) em pacotes.
// no macOS o próprio sistema converte e no Linux há um caminho só: ninguém
// implementa este contrato
#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
pub trait FrameConverter<F> {
    /// Converte e reduz o frame para caber em `max_w` x `max_h`.
    fn convert(&mut self, frame: &mut F, max_w: u32, max_h: u32) -> Result<VideoPacket, BoxError>;
}

pub trait FrameSink: Send + Sync {
    /// Um frame novo (substitui o anterior, se ainda não foi enviado).
    fn video(&self, packet: VideoPacket);
    /// Um pedaço de áudio (descartado se o destino estiver atrasado).
    fn audio(&self, packet: Bytes);
    /// O destino fechou: a captura pode parar.
    fn is_closed(&self) -> bool;
}
