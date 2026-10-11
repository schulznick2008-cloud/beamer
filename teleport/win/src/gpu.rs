//! D3D11-Geraet, DXGI-Swapchain (Flip-Modell, ohne V-Sync, Present-Queue 1) und der
//! D3D11-Video-Processor, der die dekodierte NV12/P010-Textur in den Backbuffer skaliert.

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::{Interface, Result, BOOL};
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;

/// Ein D3D11-Geraet mit Video-Unterstuetzung. Jeder Empfaenger hat sein eigenes (eigener Thread).
pub struct Gpu {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub vdev: ID3D11VideoDevice,
    pub vctx: ID3D11VideoContext,
    pub factory: IDXGIFactory2,
    pub tearing: bool,
    pub adapter_name: String,
}

fn adapter_name(a: &IDXGIAdapter1) -> String {
    unsafe { a.GetDesc1() }
        .map(|d| {
            let n = d
                .Description
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(d.Description.len());
            String::from_utf16_lossy(&d.Description[..n])
        })
        .unwrap_or_default()
}

impl Gpu {
    /// Leistungsstaerkste Hardware-GPU (auf dem Ziel-PC die RX 6600, nicht die iGPU des 4650G).
    pub fn new() -> Result<Gpu> {
        unsafe {
            let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let mut adapter: Option<IDXGIAdapter1> = None;
            for i in 0.. {
                let a: IDXGIAdapter1 = match factory
                    .EnumAdapterByGpuPreference(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
                {
                    Ok(a) => a,
                    Err(_) => break,
                };
                let d = a.GetDesc1()?;
                if (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) == 0 {
                    adapter = Some(a);
                    break;
                }
            }
            let name = adapter.as_ref().map(adapter_name).unwrap_or_default();
            let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            let mut device = None;
            let mut ctx = None;
            let mut got = D3D_FEATURE_LEVEL::default();
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
            let ad: Option<IDXGIAdapter> = adapter.as_ref().map(|a| a.cast()).transpose()?;
            D3D11CreateDevice(
                ad.as_ref(),
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                flags,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut got),
                Some(&mut ctx),
            )?;
            let device: ID3D11Device = device.unwrap();
            let ctx: ID3D11DeviceContext = ctx.unwrap();
            // FFmpeg und unser Present laufen im selben Thread; der Schutz kostet kaum etwas und
            // verhindert Ueberraschungen, falls FFmpeg intern doch einen zweiten Thread nutzt.
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let vdev: ID3D11VideoDevice = device.cast()?;
            let vctx: ID3D11VideoContext = ctx.cast()?;
            let mut tearing = BOOL(0);
            let tearing = factory
                .cast::<IDXGIFactory5>()
                .and_then(|f5| {
                    f5.CheckFeatureSupport(
                        DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                        &mut tearing as *mut _ as *mut _,
                        std::mem::size_of::<BOOL>() as u32,
                    )
                })
                .is_ok()
                && tearing.as_bool();
            Ok(Gpu {
                device,
                ctx,
                vdev,
                vctx,
                factory: factory.cast()?,
                tearing,
                adapter_name: name,
            })
        }
    }
}

struct Vp {
    src: (u32, u32),
    dst: (u32, u32),
    en: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    out_view: Option<ID3D11VideoProcessorOutputView>,
    /// Eingabe-Views je (Textur, Array-Index). FFmpegs Texturpool ist fest, die Views bleiben gueltig.
    in_views: HashMap<(usize, u32), ID3D11VideoProcessorInputView>,
}

/// Swapchain eines Proxy-Fensters.
pub struct Presenter {
    swap: IDXGISwapChain2,
    waitable: HANDLE,
    flags: u32,
    pub size: (u32, u32),
    vp: Option<Vp>,
}

/// Zielrechteck mit gleichem Seitenverhaeltnis, mittig (Rand schwarz).
pub fn letterbox(src: (u32, u32), dst: (u32, u32)) -> RECT {
    let (sw, sh) = (src.0.max(1) as f64, src.1.max(1) as f64);
    let (dw, dh) = (dst.0.max(1) as f64, dst.1.max(1) as f64);
    let s = (dw / sw).min(dh / sh);
    let (w, h) = ((sw * s).round(), (sh * s).round());
    let x = ((dw - w) / 2.0).floor();
    let y = ((dh - h) / 2.0).floor();
    RECT {
        left: x as i32,
        top: y as i32,
        right: (x + w) as i32,
        bottom: (y + h) as i32,
    }
}

impl Presenter {
    pub fn new(gpu: &Gpu, hwnd: HWND, w: u32, h: u32) -> Result<Presenter> {
        let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32;
        if gpu.tearing {
            flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32;
        }
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w.max(1),
            Height: h.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            // Waehrend der Nutzer zieht, skaliert DWM das letzte Bild, bis ResizeBuffers nachzieht.
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: flags,
        };
        unsafe {
            let swap1 = gpu
                .factory
                .CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None)?;
            let _ = gpu
                .factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let swap: IDXGISwapChain2 = swap1.cast()?;
            // Hoechstens ein Frame in der Present-Queue.
            swap.SetMaximumFrameLatency(1)?;
            let waitable = swap.GetFrameLatencyWaitableObject();
            Ok(Presenter {
                swap,
                waitable,
                flags,
                size: (w.max(1), h.max(1)),
                vp: None,
            })
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<()> {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) == self.size {
            return Ok(());
        }
        // Alle Verweise auf den Backbuffer freigeben, sonst schlaegt ResizeBuffers fehl.
        self.vp = None;
        unsafe {
            self.swap.ResizeBuffers(
                0,
                w,
                h,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(self.flags as i32),
            )?;
        }
        self.size = (w, h);
        Ok(())
    }

    fn ensure_vp(&mut self, gpu: &Gpu, src: (u32, u32)) -> Result<()> {
        if self
            .vp
            .as_ref()
            .is_some_and(|v| v.src == src && v.dst == self.size && v.out_view.is_some())
        {
            return Ok(());
        }
        self.vp = None;
        let cd = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: src.0,
            InputHeight: src.1,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: self.size.0,
            OutputHeight: self.size.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        unsafe {
            let en = gpu.vdev.CreateVideoProcessorEnumerator(&cd)?;
            let vp = gpu.vdev.CreateVideoProcessor(&en, 0)?;
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            let od = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut out_view = None;
            gpu.vdev
                .CreateVideoProcessorOutputView(&back, &en, &od, Some(&mut out_view))?;
            let v = &gpu.vctx;
            v.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            v.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
            // Eingang: BT.709, begrenzter Bereich (16-235), wie VideoToolbox/AMF es liefern.
            // Bitfeld: Usage(1) RGB_Range(1) YCbCr_Matrix(1) YCbCr_xvYCC(1) Nominal_Range(2)
            let cs_in = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (1 << 4),
            };
            v.VideoProcessorSetStreamColorSpace(&vp, 0, &cs_in);
            // Ausgang: RGB 0-255
            let cs_out = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            v.VideoProcessorSetOutputColorSpace(&vp, &cs_out);
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    RGBA: D3D11_VIDEO_COLOR_RGBA {
                        R: 0.0,
                        G: 0.0,
                        B: 0.0,
                        A: 1.0,
                    },
                },
            };
            v.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
            let full = RECT {
                left: 0,
                top: 0,
                right: self.size.0 as i32,
                bottom: self.size.1 as i32,
            };
            v.VideoProcessorSetOutputTargetRect(&vp, true, Some(&full));
            let srcr = RECT {
                left: 0,
                top: 0,
                right: src.0 as i32,
                bottom: src.1 as i32,
            };
            v.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&srcr));
            let dst = letterbox(src, self.size);
            v.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dst));
            self.vp = Some(Vp {
                src,
                dst: self.size,
                en,
                vp,
                out_view,
                in_views: HashMap::new(),
            });
        }
        Ok(())
    }

    /// Skaliert `tex[slice]` (Ausschnitt `src` Pixel) in den Backbuffer und praesentiert sofort.
    pub fn present(
        &mut self,
        gpu: &Gpu,
        tex: &ID3D11Texture2D,
        slice: u32,
        src: (u32, u32),
    ) -> Result<()> {
        self.ensure_vp(gpu, src)?;
        let vp = self.vp.as_mut().unwrap();
        let key = (tex.as_raw() as usize, slice);
        if !vp.in_views.contains_key(&key) {
            if vp.in_views.len() > 64 {
                vp.in_views.clear();
            }
            let d = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: slice,
                    },
                },
            };
            let mut view = None;
            unsafe {
                gpu.vdev
                    .CreateVideoProcessorInputView(tex, &vp.en, &d, Some(&mut view))?;
            }
            vp.in_views.insert(key, view.unwrap());
        }
        let input = vp.in_views.get(&key).cloned();
        unsafe {
            // Nicht ewig warten: haengt die Queue, lieber verspaetet praesentieren als blockieren.
            WaitForSingleObjectEx(self.waitable, 100, true);
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: ManuallyDrop::new(input),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let streams = [stream];
            let r = gpu
                .vctx
                .VideoProcessorBlt(&vp.vp, vp.out_view.as_ref().unwrap(), 0, &streams);
            let [mut s] = streams;
            ManuallyDrop::drop(&mut s.pInputSurface);
            ManuallyDrop::drop(&mut s.pInputSurfaceRight);
            r?;
            let pf = if self.flags & DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32 != 0 {
                DXGI_PRESENT_ALLOW_TEARING
            } else {
                DXGI_PRESENT(0)
            };
            // SyncInterval 0: kein V-Sync.
            self.swap.Present(0, pf).ok()?;
        }
        Ok(())
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.waitable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_keeps_aspect() {
        let r = letterbox((1920, 1080), (1920, 1080));
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 1920, 1080));
        let r = letterbox((1920, 1080), (1000, 1000));
        assert_eq!((r.left, r.right), (0, 1000));
        assert_eq!(r.bottom - r.top, 563);
        let r = letterbox((1000, 2000), (1000, 1000));
        assert_eq!((r.top, r.bottom), (0, 1000));
        assert_eq!((r.left, r.right), (250, 750));
    }
}
