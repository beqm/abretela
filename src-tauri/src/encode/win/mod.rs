//! Codificador H.264 do Media Foundation.
//!
//! Tenta primeiro os codificadores da placa de vídeo (NVENC, AMF, QuickSync),
//! que no Media Foundation são assíncronos: avisam por eventos quando querem
//! um frame (`METransformNeedInput`) e quando têm um pronto
//! (`METransformHaveOutput`). Os que pedem, recebem os frames numa textura
//! D3D11 (a placa lê direto dela). Se nenhum funcionar, usa o codificador do
//! próprio Windows no processador (síncrono, memória comum).
//!
//! Hoje o frame chega em memória comum (o mesmo NV12 que a captura já
//! produz) e é copiado para a placa. Depois dá para entregar a textura da
//! captura direto, sem passar pelo processador.

use super::{EncodedFrame, EncoderConfig, VideoEncoder};
use crate::capture::{
    contracts::BoxError,
    packet::{PixelFormat, VideoPacket},
};
use std::{
    sync::Once,
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, GUID, PWSTR},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1},
            Direct3D11::*,
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
        },
        Media::MediaFoundation::*,
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
            Variant::VARIANT,
        },
    },
};

/// Quantas texturas de entrada giram (o codificador pode ainda estar lendo a
/// anterior quando chega a próxima).
const INPUT_TEXTURES: usize = 6;
/// Quanto esperar o codificador pedir um frame antes de descartar este.
const WAIT_FOR_INPUT: Duration = Duration::from_millis(100);
/// Quanto esperar o frame codificado depois de entregar (baixa latência).
const WAIT_FOR_OUTPUT: Duration = Duration::from_millis(40);

fn startup() {
    static START: Once = Once::new();
    START.call_once(|| unsafe {
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
    });
}

/// GPU onde as texturas de entrada moram.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    _manager: IMFDXGIDeviceManager,
    staging: ID3D11Texture2D,
    pool: Vec<ID3D11Texture2D>,
    next: usize,
}

pub struct MfEncoder {
    transform: IMFTransform,
    codec: Option<ICodecAPI>,
    /// Só nos assíncronos (placa de vídeo).
    events: Option<IMFMediaEventGenerator>,
    gpu: Option<Gpu>,
    name: String,
    hardware: bool,
    config: EncoderConfig,
    /// O codificador fornece as amostras de saída (senão a gente cria).
    provides_samples: bool,
    output_size: u32,
    /// Pedidos de frame (`METransformNeedInput`) ainda não atendidos.
    wants_input: u32,
    dropped: u64,
}

// SAFETY: os objetos COM foram criados numa thread MTA (multithreaded) e o
// codificador é usado por uma thread de cada vez (`&mut self`).
unsafe impl Send for MfEncoder {}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn friendly_name(activate: &IMFActivate) -> String {
    unsafe {
        let mut p = PWSTR::null();
        let mut len = 0;
        if activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len).is_ok() && !p.is_null() {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as _));
            return s;
        }
    }
    "unknown encoder".into()
}

/// Os codificadores H.264 registrados, do melhor para o pior.
fn candidates(hardware: bool) -> Vec<IMFActivate> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let flags = if hardware {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
    } else {
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    let mut out = Vec::new();
    unsafe {
        let ok = MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count);
        if ok.is_err() || list.is_null() {
            return out;
        }
        for i in 0..count as usize {
            if let Some(a) = (*list.add(i)).take() {
                out.push(a);
            }
        }
        CoTaskMemFree(Some(list as _));
    }
    out
}

impl MfEncoder {
    pub fn open(config: EncoderConfig) -> Result<Self, BoxError> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        startup();
        let mut errors = Vec::new();
        let tiers: &[bool] = if config.software_only { &[false] } else { &[true, false] };
        for &hardware in tiers {
            for activate in candidates(hardware) {
                let name = friendly_name(&activate);
                match Self::setup(&activate, &name, hardware, config) {
                    Ok(enc) => {
                        if !errors.is_empty() {
                            log_warn!("encode", "Skipped encoders that failed to start: {}", errors.join("; "));
                        }
                        log_info!(
                            "encode",
                            "Using {} for {}x{} at {} fps, {} kbps",
                            enc.describe(),
                            config.width,
                            config.height,
                            config.fps,
                            config.bitrate / 1000
                        );
                        return Ok(enc);
                    }
                    Err(e) => {
                        errors.push(format!("{name}: {e}"));
                        unsafe {
                            let _ = activate.ShutdownObject();
                        }
                    }
                }
            }
        }
        Err(format!("no H.264 encoder could start ({})", errors.join("; ")).into())
    }

    fn setup(activate: &IMFActivate, name: &str, hardware: bool, config: EncoderConfig) -> Result<Self, BoxError> {
        unsafe {
            let transform: IMFTransform = activate.ActivateObject()?;
            let attrs = transform.GetAttributes().ok();
            let is_async = attrs.as_ref().and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok()) == Some(1);
            if is_async {
                attrs.as_ref().unwrap().SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
            let d3d_aware = attrs.as_ref().and_then(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).ok()) == Some(1);
            let gpu = if hardware && d3d_aware { Some(Self::gpu(&transform, config)?) } else { None };

            let codec: Option<ICodecAPI> = transform.cast().ok();
            Self::tune(codec.as_ref(), config);

            // a saída primeiro (os codificadores exigem essa ordem)
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(config.width, config.height))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(config.fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            // Constrained Baseline: todo navegador decodifica (o mesmo 42e01f do protótipo)
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_ConstrainedBase.0 as u32)?;
            transform.SetOutputType(0, &out, 0)?;

            let input = Self::nv12_input_type(&transform, config)?;
            transform.SetInputType(0, &input, 0)?;

            // opções antes e depois dos formatos: o codificador do Windows só
            // aceita baixa latência antes; alguns da placa só depois
            Self::tune(codec.as_ref(), config);

            let info = transform.GetOutputStreamInfo(0)?;
            let provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                != 0;

            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let events = if is_async { Some(transform.cast::<IMFMediaEventGenerator>()?) } else { None };
            let place = match (hardware, &gpu) {
                (true, Some(_)) => "graphics card",
                (true, None) => "graphics card, frames from memory",
                _ => "processor",
            };
            Ok(Self {
                transform,
                codec,
                events,
                gpu,
                name: format!("{name} ({place})"),
                hardware,
                config,
                provides_samples,
                output_size: info.cbSize.max(config.width * config.height),
                wants_input: 0,
                dropped: 0,
            })
        }
    }

    /// Baixa latência, sem quadros B, bitrate e quadro-chave só quando pedido.
    fn tune(codec: Option<&ICodecAPI>, config: EncoderConfig) {
        let Some(c) = codec else { return };
        let set = |api: &GUID, v: VARIANT| unsafe { c.SetValue(api, &v).is_ok() };
        set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
        set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
        if !set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_LowDelayVBR.0 as u32)) {
            set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32));
        }
        set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(config.bitrate));
        // quadro-chave só quando alguém pede (quem entra, ou perda)
        set(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(config.fps * 60 * 10));
    }

    /// Placa de vídeo para as texturas de entrada, entregue ao codificador.
    fn gpu(transform: &IMFTransform, config: EncoderConfig) -> Result<Gpu, BoxError> {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or("no D3D11 device")?;
            let context = context.ok_or("no D3D11 context")?;
            // o codificador usa o device em outra thread
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or("no DXGI device manager")?;
            manager.ResetDevice(&device, token)?;
            transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: config.width,
                Height: config.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            desc.Usage = D3D11_USAGE_DEFAULT;
            desc.CPUAccessFlags = 0;
            let mut pool = Vec::new();
            for _ in 0..INPUT_TEXTURES {
                let mut tex = None;
                desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
                if device.CreateTexture2D(&desc, None, Some(&mut tex)).is_err() {
                    desc.BindFlags = 0;
                    device.CreateTexture2D(&desc, None, Some(&mut tex))?;
                }
                pool.push(tex.ok_or("no texture")?);
            }
            Ok(Gpu { device, context, _manager: manager, staging: staging.ok_or("no staging texture")?, pool, next: 0 })
        }
    }

    /// O tipo de entrada NV12 que o codificador oferece, no tamanho certo.
    fn nv12_input_type(transform: &IMFTransform, config: EncoderConfig) -> Result<IMFMediaType, BoxError> {
        unsafe {
            let mut i = 0;
            let found = loop {
                match transform.GetInputAvailableType(0, i) {
                    Ok(t) if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) => break Some(t),
                    Ok(_) => i += 1,
                    Err(_) => break None,
                }
            };
            let t = match found {
                Some(t) => t,
                None => {
                    let t = MFCreateMediaType()?;
                    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                    t
                }
            };
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(config.width, config.height))?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(config.fps, 1))?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            Ok(t)
        }
    }

    /// Monta a amostra de entrada (textura na placa ou memória comum).
    fn input_sample(&mut self, frame: &VideoPacket, timestamp: Duration) -> Result<IMFSample, BoxError> {
        let (w, h) = (self.config.width as usize, self.config.height as usize);
        let src = frame.pixels();
        let nv12 = frame.format() == PixelFormat::Nv12;
        // I420 -> NV12: o plano Y é igual; U e V viram um plano intercalado
        let write = |dst: *mut u8, pitch: usize| unsafe {
            for row in 0..h {
                std::ptr::copy_nonoverlapping(src.as_ptr().add(row * w), dst.add(row * pitch), w);
            }
            let uv_dst = dst.add(pitch * h);
            if nv12 {
                for row in 0..h / 2 {
                    std::ptr::copy_nonoverlapping(src.as_ptr().add(w * h + row * w), uv_dst.add(row * pitch), w);
                }
            } else {
                let (u, v) = (w * h, w * h + w * h / 4);
                for row in 0..h / 2 {
                    let line = uv_dst.add(row * pitch);
                    for col in 0..w / 2 {
                        *line.add(col * 2) = src[u + row * w / 2 + col];
                        *line.add(col * 2 + 1) = src[v + row * w / 2 + col];
                    }
                }
            }
        };
        unsafe {
            let sample = MFCreateSample()?;
            let buffer = if let Some(gpu) = &mut self.gpu {
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                gpu.context.Map(&gpu.staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))?;
                write(mapped.pData as *mut u8, mapped.RowPitch as usize);
                gpu.context.Unmap(&gpu.staging, 0);
                let tex = gpu.pool[gpu.next].clone();
                gpu.next = (gpu.next + 1) % gpu.pool.len();
                gpu.context.CopyResource(&tex, &gpu.staging);
                let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &tex, 0, false)?;
                if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
                    buffer.SetCurrentLength(b2.GetContiguousLength()?)?;
                }
                let _ = &gpu.device;
                buffer
            } else {
                let len = (w * h * 3 / 2) as u32;
                let buffer = MFCreateMemoryBuffer(len)?;
                let mut ptr = std::ptr::null_mut();
                buffer.Lock(&mut ptr, None, None)?;
                write(ptr, w);
                buffer.Unlock()?;
                buffer.SetCurrentLength(len)?;
                buffer
            };
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime((timestamp.as_nanos() / 100) as i64)?;
            sample.SetSampleDuration(10_000_000 / self.config.fps as i64)?;
            Ok(sample)
        }
    }

    /// Pega um frame codificado. `Ok(None)` = ainda não há.
    fn pull_output(&mut self) -> Result<Option<EncodedFrame>, BoxError> {
        unsafe {
            let own = if self.provides_samples {
                None
            } else {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(self.output_size)?)?;
                Some(s)
            };
            let mut buf = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(own),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            };
            let mut status = 0;
            let result = self.transform.ProcessOutput(0, std::slice::from_mut(&mut buf), &mut status);
            let sample = std::mem::ManuallyDrop::into_inner(buf.pSample);
            drop(std::mem::ManuallyDrop::into_inner(buf.pEvents));
            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // o codificador mudou o formato de saída: aceita o novo
                    let t = self.transform.GetOutputAvailableType(0, 0)?;
                    self.transform.SetOutputType(0, &t, 0)?;
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
            let Some(sample) = sample else { return Ok(None) };
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut ptr, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            buffer.Unlock()?;
            let keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint).ok() == Some(1) || super::has_idr(&data);
            let timestamp = Duration::from_nanos(sample.GetSampleTime().unwrap_or(0).max(0) as u64 * 100);
            Ok(Some(EncodedFrame { data, keyframe, timestamp }))
        }
    }

    /// Lê os eventos do codificador assíncrono sem esperar.
    fn poll_events(&mut self, out: &mut Vec<EncodedFrame>) -> Result<(), BoxError> {
        let Some(events) = self.events.clone() else { return Ok(()) };
        loop {
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            let kind = MF_EVENT_TYPE(unsafe { event.GetType()? } as i32);
            if kind == METransformNeedInput {
                self.wants_input += 1;
            } else if kind == METransformHaveOutput {
                if let Some(frame) = self.pull_output()? {
                    out.push(frame);
                }
            } else if kind == MEError {
                let status = unsafe { event.GetStatus() }.map(|s| format!("{s:?}")).unwrap_or_default();
                return Err(format!("the encoder reported an error {status}").into());
            }
        }
    }

    fn encode_async(&mut self, sample: &IMFSample, out: &mut Vec<EncodedFrame>) -> Result<(), BoxError> {
        let start = Instant::now();
        self.poll_events(out)?;
        while self.wants_input == 0 {
            if start.elapsed() > WAIT_FOR_INPUT {
                self.dropped += 1;
                if self.dropped.is_power_of_two() {
                    log_warn!("encode", "The encoder is not keeping up, dropped {} frames so far", self.dropped);
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
            self.poll_events(out)?;
        }
        unsafe { self.transform.ProcessInput(0, sample, 0)? };
        self.wants_input -= 1;
        let produced = out.len();
        let start = Instant::now();
        while out.len() == produced && start.elapsed() < WAIT_FOR_OUTPUT {
            self.poll_events(out)?;
            if out.len() == produced {
                std::thread::sleep(Duration::from_micros(500));
            }
        }
        Ok(())
    }

    fn encode_sync(&mut self, sample: &IMFSample, out: &mut Vec<EncodedFrame>) -> Result<(), BoxError> {
        unsafe { self.transform.ProcessInput(0, sample, 0)? };
        while let Some(frame) = self.pull_output()? {
            out.push(frame);
        }
        Ok(())
    }
}

impl VideoEncoder for MfEncoder {
    fn describe(&self) -> String {
        self.name.clone()
    }

    fn hardware(&self) -> bool {
        self.hardware
    }

    fn encode(&mut self, frame: &VideoPacket, timestamp: Duration, keyframe: bool) -> Result<Vec<EncodedFrame>, BoxError> {
        if (frame.width() as u32, frame.height() as u32) != (self.config.width, self.config.height) {
            return Err(format!(
                "frame is {}x{} but the encoder was opened for {}x{}",
                frame.width(),
                frame.height(),
                self.config.width,
                self.config.height
            )
            .into());
        }
        if keyframe {
            if let Some(c) = &self.codec {
                unsafe {
                    let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
                }
            }
        }
        let sample = self.input_sample(frame, timestamp)?;
        let mut out = Vec::new();
        if self.events.is_some() {
            self.encode_async(&sample, &mut out)?;
        } else {
            self.encode_sync(&sample, &mut out)?;
        }
        Ok(out)
    }

    fn set_bitrate(&mut self, bps: u32) {
        self.config.bitrate = bps;
        if let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bps));
            }
        }
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
        if self.dropped > 0 {
            log_info!("encode", "{} closed; {} frames were dropped because it was busy", self.name, self.dropped);
        }
    }
}
