//! Redimensiona e converte o frame capturado (BGRA) para NV12 na placa de
//! vídeo, com o processador de vídeo do D3D11. Assim o processador só copia
//! o resultado já pequeno (1,5 byte por pixel, na resolução final) em vez do
//! frame inteiro em BGRA, e não faz conversão de cor nenhuma.

use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// D3D11_VIDEO_PROCESSOR_COLOR_SPACE: YCbCr_Matrix = BT.709 (bit 2),
/// Nominal_Range = 16-235 (bits 4-5 = 1).
const OUT_BT709_LIMITED: u32 = (1 << 2) | (1 << 4);
/// RGB de 0 a 255 (todos os bits zerados).
const IN_RGB_FULL: u32 = 0;
const FORMAT_SUPPORT_OUTPUT: u32 = 0x2;

pub struct GpuConverter {
    src: (u32, u32),
    pub dst: (u32, u32),
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output_view: ID3D11VideoProcessorOutputView,
    output: ID3D11Texture2D,
    staging: ID3D11Texture2D,
}

impl GpuConverter {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        src: (u32, u32),
        dst: (u32, u32),
    ) -> Result<Self, BoxError> {
        unsafe {
            let video_device: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;

            let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: src.0,
                InputHeight: src.1,
                OutputFrameRate: rate,
                OutputWidth: dst.0,
                OutputHeight: dst.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            if enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)? & FORMAT_SUPPORT_OUTPUT == 0 {
                return Err("the GPU video processor can't output NV12".into());
            }
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;

            let tex_desc = |usage, bind: u32, cpu: u32| D3D11_TEXTURE2D_DESC {
                Width: dst.0,
                Height: dst.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: usage,
                BindFlags: bind,
                CPUAccessFlags: cpu,
                MiscFlags: 0,
            };
            let mut output = None;
            device.CreateTexture2D(
                &tex_desc(D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0),
                None,
                Some(&mut output),
            )?;
            let output = output.ok_or("no output texture")?;
            let mut staging = None;
            device.CreateTexture2D(
                &tex_desc(D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32),
                None,
                Some(&mut staging),
            )?;
            let staging = staging.ok_or("no staging texture")?;

            let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut output_view = None;
            video_device.CreateVideoProcessorOutputView(&output, &enumerator, &view_desc, Some(&mut output_view))?;
            let output_view = output_view.ok_or("no output view")?;

            let full = |w: u32, h: u32| RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
            video_context.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video_context.VideoProcessorSetStreamColorSpace(
                &processor,
                0,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: IN_RGB_FULL },
            );
            video_context.VideoProcessorSetOutputColorSpace(
                &processor,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: OUT_BT709_LIMITED },
            );
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&full(src.0, src.1)));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&full(dst.0, dst.1)));
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&full(dst.0, dst.1)));

            Ok(Self { src, dst, video_device, video_context, enumerator, processor, output_view, output, staging })
        }
    }

    pub fn fits(&self, src: (u32, u32), dst: (u32, u32)) -> bool {
        self.src == src && self.dst == dst
    }

    /// Converte `texture` e escreve o NV12 (sem padding) em `out`, que deve
    /// ter `largura * altura * 3 / 2` bytes.
    pub fn convert(
        &self,
        context: &ID3D11DeviceContext,
        texture: &ID3D11Texture2D,
        out: &mut [u8],
    ) -> Result<(), BoxError> {
        let (w, h) = (self.dst.0 as usize, self.dst.1 as usize);
        unsafe {
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut input_view = None;
            self.video_device.CreateVideoProcessorInputView(
                texture,
                &self.enumerator,
                &in_desc,
                Some(&mut input_view),
            )?;

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(input_view),
                ..Default::default()
            };
            let mut streams = [stream];
            let blt = self.video_context.VideoProcessorBlt(&self.processor, &self.output_view, 0, &streams);
            std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
            blt?;

            context.CopyResource(&self.staging, &self.output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let base = mapped.pData as *const u8;
            // plano Y (h linhas) seguido do plano UV intercalado (h/2 linhas)
            for row in 0..h + h / 2 {
                let src = std::slice::from_raw_parts(base.add(row * pitch), w);
                out[row * w..][..w].copy_from_slice(src);
            }
            context.Unmap(&self.staging, 0);
        }
        Ok(())
    }
}
