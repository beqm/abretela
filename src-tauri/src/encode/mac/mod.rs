//! Codificador H.264 do macOS (VideoToolbox).
//!
//! Uma `VTCompressionSession` em tempo real, com o controle de banda de baixa
//! latência do chip (sem segurar frames nem reordenar), Constrained Baseline e
//! quadro-chave só quando pedido. Com `software_only`, o codificador do
//! próprio macOS no processador.
//!
//! O VideoToolbox já trabalha em fila: `encode` copia o frame para um buffer
//! do sistema, entrega e volta; cada frame pronto chega por outra thread em
//! [`output`], que o converte de AVCC (cada NAL precedido do tamanho) para
//! Annex B e chama [`OnEncoded`].

use super::{EncodedFrame, EncoderConfig, OnEncoded, VideoEncoder};
use crate::capture::{
    contracts::BoxError,
    packet::{PixelFormat, VideoPacket},
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMTimeInvalid, kCMVideoCodecType_H264, CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::{
    kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_709_2,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ColorPrimaries, kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_MaxKeyFrameInterval, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, kVTCompressionPropertyKey_YCbCrMatrix,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_Baseline_AutoLevel,
    kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl, VTCompressionSession, VTEncodeInfoFlags,
    VTSessionCopyProperty, VTSessionSetProperty,
};
use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Quantos frames podem estar dentro do codificador ao mesmo tempo. Acima
/// disso ele está atrasado, e o frame novo é descartado em vez de entrar na
/// fila (o próximo da captura chega logo).
const MAX_INSIDE: u32 = 3;
/// O tempo dos frames vai em nanossegundos, para voltar igual ao que entrou.
const NANOS: i32 = 1_000_000_000;

type Dictionary = CFDictionary<CFString, CFType>;

fn dictionary(pairs: &[(&CFString, &CFType)]) -> CFRetained<Dictionary> {
    let keys: Vec<&CFString> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<&CFType> = pairs.iter().map(|p| p.1).collect();
    CFDictionary::from_slices(&keys, &values)
}

/// O que `encode` e a thread do VideoToolbox dividem.
struct Shared {
    on_encoded: OnEncoded,
    /// De quando contam os tempos de entrada dos frames.
    started: Instant,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Frames entregues que ainda não saíram.
    inside: u32,
    timings: Timings,
}

/// Tempo somado de cada etapa, para o log.
#[derive(Default)]
struct Timings {
    frames: u32,
    copy: Duration,
    /// Do frame entrar até sair codificado.
    latency: Duration,
    out: u32,
    /// Descartados porque o codificador ainda estava ocupado.
    dropped: u32,
    /// Descartados pelo próprio codificador, para não passar do bitrate.
    held_back: u32,
}

pub struct VtEncoder {
    session: CFRetained<VTCompressionSession>,
    /// O VideoToolbox guarda um ponteiro para isto (`output`): só pode ser
    /// solto depois de `invalidate`.
    shared: Arc<Shared>,
    /// Formato dos buffers de entrada, se o codificador não emprestar os dele.
    buffer_attributes: CFRetained<Dictionary>,
    name: String,
    hardware: bool,
    config: EncoderConfig,
}

// SAFETY: a sessão do VideoToolbox pode ser usada de qualquer thread (ele
// mesmo devolve os frames por outra), e aqui só uma a usa de cada vez
// (`&mut self`).
unsafe impl Send for VtEncoder {}

impl VtEncoder {
    pub fn open(config: EncoderConfig, on_encoded: OnEncoded) -> Result<Self, BoxError> {
        let shared = Arc::new(Shared { on_encoded, started: Instant::now(), state: Mutex::default() });
        let (width, height) = (CFNumber::new_i32(config.width as i32), CFNumber::new_i32(config.height as i32));
        let format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
        // um dicionário vazio aqui pede buffers que o chip lê sem cópia (IOSurface)
        let surface = dictionary(&[]);
        let buffer_attributes = unsafe {
            dictionary(&[
                (kCVPixelBufferPixelFormatTypeKey, &format),
                (kCVPixelBufferWidthKey, &width),
                (kCVPixelBufferHeightKey, &height),
                (kCVPixelBufferIOSurfacePropertiesKey, &surface),
            ])
        };

        // pelo chip com baixa latência; se este Mac não tiver, pelo chip sem
        // ela; por último (ou a pedido), no processador
        let yes: &CFType = CFBoolean::new(true);
        let no: &CFType = CFBoolean::new(false);
        let specifications = unsafe {
            if config.software_only {
                vec![dictionary(&[(kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, no)])]
            } else {
                vec![
                    dictionary(&[(kVTVideoEncoderSpecification_EnableLowLatencyRateControl, yes)]),
                    dictionary(&[(kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, yes)]),
                ]
            }
        };
        let mut session = None;
        let mut status = 0;
        let mut low_latency = false;
        for (index, specification) in specifications.iter().enumerate() {
            let mut raw: *mut VTCompressionSession = std::ptr::null_mut();
            let specification: &CFDictionary = specification.as_ref();
            let attributes: &CFDictionary = buffer_attributes.as_ref();
            status = unsafe {
                VTCompressionSession::create(
                    None,
                    config.width as i32,
                    config.height as i32,
                    kCMVideoCodecType_H264,
                    Some(specification),
                    Some(attributes),
                    None,
                    Some(output),
                    Arc::as_ptr(&shared) as *mut c_void,
                    NonNull::from(&mut raw),
                )
            };
            if let (0, Some(raw)) = (status, NonNull::new(raw)) {
                session = Some(unsafe { CFRetained::from_raw(raw) });
                low_latency = !config.software_only && index == 0;
                break;
            }
        }
        let Some(session) = session else {
            return Err(format!("VideoToolbox could not open an H.264 encoder (error {status})").into());
        };

        let mut encoder =
            Self { session, shared, buffer_attributes, name: String::new(), hardware: false, config };
        encoder.tune();
        // a baixa latência só existe no chip
        encoder.hardware = encoder.uses_hardware().unwrap_or(low_latency);
        encoder.name =
            format!("VideoToolbox H.264 ({})", if encoder.hardware { "graphics chip" } else { "processor" });
        let status = unsafe { encoder.session.prepare_to_encode_frames() };
        if status != 0 {
            return Err(format!("VideoToolbox could not start the H.264 encoder (error {status})").into());
        }
        Ok(encoder)
    }

    fn set(&self, key: &CFString, value: &CFType) -> bool {
        unsafe { VTSessionSetProperty(&self.session, key, Some(value)) == 0 }
    }

    /// Tempo real, sem quadros B, bitrate e quadro-chave só quando pedido.
    fn tune(&self) {
        let config = self.config;
        let yes: &CFType = CFBoolean::new(true);
        let no: &CFType = CFBoolean::new(false);
        unsafe {
            self.set(kVTCompressionPropertyKey_RealTime, yes);
            self.set(kVTCompressionPropertyKey_AllowFrameReordering, no);
            // Constrained Baseline: todo navegador decodifica (o mesmo 42e01f do Windows)
            if !self.set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel) {
                self.set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Baseline_AutoLevel);
            }
            self.set(kVTCompressionPropertyKey_AverageBitRate, &CFNumber::new_i32(config.bitrate as i32));
            self.set(kVTCompressionPropertyKey_ExpectedFrameRate, &CFNumber::new_i32(config.fps as i32));
            // quadro-chave só quando alguém pede (quem entra, ou perda)
            self.set(kVTCompressionPropertyKey_MaxKeyFrameInterval, &CFNumber::new_i32((config.fps * 60 * 10) as i32));
            // as cores da captura (BT.709), para quem decodifica não adivinhar
            self.set(kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
            self.set(kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
            self.set(kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
        }
    }

    /// O codificador aberto usa o chip? `None` se ele não diz (no modo de
    /// baixa latência a pergunta não é aceita).
    fn uses_hardware(&self) -> Option<bool> {
        let mut value: *const CFBoolean = std::ptr::null();
        let status = unsafe {
            VTSessionCopyProperty(
                &self.session,
                kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                None,
                (&mut value as *mut *const CFBoolean).cast(),
            )
        };
        match (status, NonNull::new(value as *mut CFBoolean)) {
            (0, Some(value)) => Some(unsafe { CFRetained::from_raw(value) }.as_bool()),
            _ => None,
        }
    }

    /// Um buffer do sistema com os pixels do frame (NV12).
    fn pixel_buffer(&self, frame: &VideoPacket) -> Result<CFRetained<CVPixelBuffer>, BoxError> {
        let (w, h) = (self.config.width as usize, self.config.height as usize);
        let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
        // o codificador empresta buffers que ele reaproveita; sem isso, cria um
        let status = match unsafe { self.session.pixel_buffer_pool() } {
            Some(pool) => unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut raw)) },
            None => {
                let attributes: &CFDictionary = self.buffer_attributes.as_ref();
                unsafe {
                    CVPixelBufferCreate(
                        None,
                        w,
                        h,
                        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                        Some(attributes),
                        NonNull::from(&mut raw),
                    )
                }
            }
        };
        let Some(raw) = NonNull::new(raw).filter(|_| status == 0) else {
            return Err(format!("could not get a buffer for the frame (error {status})").into());
        };
        let buffer = unsafe { CFRetained::from_raw(raw) };

        let flags = CVPixelBufferLockFlags(0);
        let status = unsafe { CVPixelBufferLockBaseAddress(&buffer, flags) };
        if status != 0 {
            return Err(format!("could not write to the buffer of the frame (error {status})").into());
        }
        let (luma, chroma) = frame.pixels().split_at(w * h);
        unsafe {
            let plane = |index: usize, rows: usize| {
                let stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, index);
                let base = CVPixelBufferGetBaseAddressOfPlane(&buffer, index) as *mut u8;
                std::slice::from_raw_parts_mut(base, stride * rows).chunks_exact_mut(stride)
            };
            for (dst, src) in plane(0, h).zip(luma.chunks_exact(w)) {
                dst[..w].copy_from_slice(src);
            }
            match frame.format() {
                PixelFormat::Nv12 => {
                    for (dst, src) in plane(1, h / 2).zip(chroma.chunks_exact(w)) {
                        dst[..w].copy_from_slice(src);
                    }
                }
                // U e V em planos separados: intercala
                PixelFormat::I420 => {
                    let (u, v) = chroma.split_at(w * h / 4);
                    for ((dst, u), v) in plane(1, h / 2).zip(u.chunks_exact(w / 2)).zip(v.chunks_exact(w / 2)) {
                        for ((pair, u), v) in dst.chunks_exact_mut(2).zip(u).zip(v) {
                            pair[0] = *u;
                            pair[1] = *v;
                        }
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(&buffer, flags);
        }
        Ok(buffer)
    }
}

/// O VideoToolbox chama aqui, por uma thread dele e em ordem, a cada frame
/// que termina (ou que ele descartou).
unsafe extern "C-unwind" fn output(
    shared: *mut c_void,
    entered: *mut c_void,
    status: i32,
    flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    let shared = &*(shared as *const Shared);
    // `encode` manda no lugar do ponteiro quando o frame entrou
    let entered = Duration::from_nanos(entered as usize as u64);
    let frame = match sample.as_ref() {
        Some(sample) if status == 0 && !flags.contains(VTEncodeInfoFlags::FrameDropped) => encoded_frame(sample),
        _ => None,
    };
    {
        let mut state = shared.state.lock().unwrap();
        state.inside = state.inside.saturating_sub(1);
        match &frame {
            Some(_) => {
                state.timings.out += 1;
                state.timings.latency += shared.started.elapsed().saturating_sub(entered);
            }
            None => state.timings.held_back += 1,
        }
    }
    if let Some(frame) = frame {
        (shared.on_encoded)(frame);
    }
}

/// Tira o H.264 de uma amostra do VideoToolbox, em Annex B. Nos
/// quadros-chave, os parâmetros (SPS e PPS) vão na frente: no AVCC eles ficam
/// fora do frame, na descrição do formato.
unsafe fn encoded_frame(sample: &CMSampleBuffer) -> Option<EncodedFrame> {
    let block = sample.data_buffer()?;
    let mut avcc = vec![0u8; block.data_length()];
    if block.copy_data_bytes(0, avcc.len(), NonNull::new(avcc.as_mut_ptr().cast())?) != 0 {
        return None;
    }
    let format = sample.format_description()?;
    let (mut sets, mut length_size) = (0usize, 0i32);
    let (no_set, no_size) = (std::ptr::null_mut(), std::ptr::null_mut());
    if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(&format, 0, no_set, no_size, &mut sets, &mut length_size) != 0 {
        return None;
    }
    let (frame, keyframe) = avcc_to_annex_b(&avcc, length_size as usize);
    let mut data = Vec::with_capacity(frame.len() + 64);
    if keyframe {
        for index in 0..sets {
            let (mut set, mut size) = (std::ptr::null(), 0usize);
            let (no_count, no_length) = (std::ptr::null_mut(), std::ptr::null_mut());
            if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(&format, index, &mut set, &mut size, no_count, no_length)
                != 0
            {
                return None;
            }
            data.extend_from_slice(&START_CODE);
            data.extend_from_slice(std::slice::from_raw_parts(set, size));
        }
    }
    data.extend_from_slice(&frame);
    let time = sample.presentation_time_stamp();
    Some(EncodedFrame { data, keyframe, timestamp: Duration::from_nanos(time.value.max(0) as u64) })
}

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// AVCC (cada NAL precedido do tamanho, em `length_size` bytes) para Annex B
/// (cada NAL precedido de `00 00 00 01`). Também diz se há um quadro IDR.
fn avcc_to_annex_b(avcc: &[u8], length_size: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(avcc.len() + 16);
    let mut keyframe = false;
    let mut rest = avcc;
    while (1..=4).contains(&length_size) && rest.len() > length_size {
        let (length, after) = rest.split_at(length_size);
        let length = length.iter().fold(0usize, |total, &byte| (total << 8) | byte as usize);
        if length == 0 || length > after.len() {
            break;
        }
        let (nal, after) = after.split_at(length);
        keyframe |= nal[0] & 0x1f == 5;
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
        rest = after;
    }
    (out, keyframe)
}

impl VideoEncoder for VtEncoder {
    fn describe(&self) -> String {
        self.name.clone()
    }

    fn hardware(&self) -> bool {
        self.hardware
    }

    fn encode(&mut self, frame: &VideoPacket, timestamp: Duration, keyframe: bool) -> Result<(), BoxError> {
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
        {
            let mut state = self.shared.state.lock().unwrap();
            state.timings.frames += 1;
            // um quadro-chave pedido nunca é descartado: alguém está esperando por ele
            if state.inside >= MAX_INSIDE && !keyframe {
                state.timings.dropped += 1;
                return Ok(());
            }
        }
        let t = Instant::now();
        let buffer = self.pixel_buffer(frame)?;
        let copy = t.elapsed();

        let options = keyframe
            .then(|| unsafe { dictionary(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, CFBoolean::new(true))]) });
        let options: Option<&CFDictionary> = options.as_ref().map(|o| o.as_ref());
        let entered = self.shared.started.elapsed().as_nanos() as usize;
        {
            let mut state = self.shared.state.lock().unwrap();
            state.timings.copy += copy;
            state.inside += 1;
        }
        let status = unsafe {
            self.session.encode_frame(
                &buffer,
                CMTime::new(timestamp.as_nanos() as i64, NANOS),
                kCMTimeInvalid,
                options,
                entered as *mut c_void,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            // não entrou: `output` não vai ser chamado para este frame
            let mut state = self.shared.state.lock().unwrap();
            state.inside = state.inside.saturating_sub(1);
            return Err(format!("VideoToolbox refused the frame (error {status})").into());
        }
        Ok(())
    }

    fn set_bitrate(&mut self, bps: u32) {
        self.config.bitrate = bps;
        unsafe {
            self.set(kVTCompressionPropertyKey_AverageBitRate, &CFNumber::new_i32(bps as i32));
        }
    }

    fn take_timings(&mut self) -> Option<String> {
        let t = std::mem::take(&mut self.shared.state.lock().unwrap().timings);
        let per = |d: Duration, n: u32| d.as_secs_f64() * 1000.0 / n.max(1) as f64;
        Some(format!(
            "{} frames out ({} skipped because the encoder was still busy, {} left out by the encoder to keep \
             the bitrate), copy to the encoder {:.1} ms, from frame in to frame out {:.1} ms",
            t.out,
            t.dropped,
            t.held_back,
            per(t.copy, t.frames),
            per(t.latency, t.out)
        ))
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        unsafe {
            // espera os frames que ainda estão dentro e fecha: depois disto o
            // VideoToolbox não chama mais `output` (que usa `shared`)
            self.session.complete_frames(kCMTimeInvalid);
            self.session.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::has_idr;

    #[test]
    fn converts_avcc_to_annex_b_and_finds_the_keyframe() {
        // SEI (6) e IDR (5), com o tamanho em 4 bytes
        let avcc = [0, 0, 0, 2, 0x06, 0xAA, 0, 0, 0, 3, 0x65, 0xBB, 0xCC];
        let (annex_b, keyframe) = avcc_to_annex_b(&avcc, 4);
        assert_eq!(annex_b, [0, 0, 0, 1, 0x06, 0xAA, 0, 0, 0, 1, 0x65, 0xBB, 0xCC]);
        assert!(keyframe && has_idr(&annex_b));

        // um frame comum (1), com o tamanho em 2 bytes
        let (annex_b, keyframe) = avcc_to_annex_b(&[0, 2, 0x41, 0x99], 2);
        assert_eq!(annex_b, [0, 0, 0, 1, 0x41, 0x99]);
        assert!(!keyframe && !has_idr(&annex_b));
    }

    #[test]
    fn stops_at_a_broken_length_instead_of_reading_past_the_end() {
        // o segundo NAL diz ter 9 bytes, mas só há 1
        let (annex_b, keyframe) = avcc_to_annex_b(&[0, 0, 0, 1, 0x41, 0, 0, 0, 9, 0x65], 4);
        assert_eq!(annex_b, [0, 0, 0, 1, 0x41]);
        assert!(!keyframe);
        assert_eq!(avcc_to_annex_b(&[0, 0, 0, 1, 0x65], 0), (Vec::new(), false));
        assert_eq!(avcc_to_annex_b(&[], 4), (Vec::new(), false));
    }
}
