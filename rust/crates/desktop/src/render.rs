use anyhow::Result;
use windows::{
    core::{w, Interface},
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::*,
            DirectWrite::*,
            Dxgi::{Common::*, *},
        },
    },
};
pub struct Renderer {
    pub dc: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    composition: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    factory: ID2D1Factory1,
    format: IDWriteTextFormat,
    small: IDWriteTextFormat,
    title: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    size: (u32, u32),
}
pub fn color(hex: u32, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 255) as f32 / 255.,
        g: ((hex >> 8) & 255) as f32 / 255.,
        b: (hex & 255) as f32 / 255.,
        a: alpha,
    }
}
pub fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}
pub fn point(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}
impl Renderer {
    pub fn new(hwnd: HWND, width: u32, height: u32, dpi: f32) -> Result<Self> {
        unsafe {
            let mut device = None;
            let create = |driver, device: &mut Option<ID3D11Device>| {
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(device),
                    None,
                    None,
                )
            };
            if create(D3D_DRIVER_TYPE_HARDWARE, &mut device).is_err() {
                create(D3D_DRIVER_TYPE_WARP, &mut device)?;
            }
            let device = device.unwrap();
            let dxgi: IDXGIDevice = device.cast()?;
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let swap = factory.CreateSwapChainForComposition(
                &device,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: width,
                    Height: height,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    Stereo: false.into(),
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    Scaling: DXGI_SCALING_STRETCH,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                    AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                    Flags: 0,
                },
                None,
            )?;
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let dc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetDpi(dpi, dpi);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = composition.CreateTargetForHwnd(hwnd, true)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;
            let fonts: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let make = |size, weight| -> windows::core::Result<IDWriteTextFormat> {
                let f = fonts.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("zh-CN"),
                )?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                Ok(f)
            };
            let format = make(13., DWRITE_FONT_WEIGHT_NORMAL)?;
            let small = make(11., DWRITE_FONT_WEIGHT_NORMAL)?;
            let title = make(13., DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let brush = dc.CreateSolidColorBrush(&color(0xffffff, 1.), None)?;
            let mut result = Self {
                dc,
                swap,
                composition,
                _target: target,
                _visual: visual,
                factory,
                format,
                small,
                title,
                brush,
                size: (width, height),
            };
            result.target(dpi)?;
            Ok(result)
        }
    }
    unsafe fn target(&mut self, dpi: f32) -> Result<()> {
        let surface: IDXGISurface = self.swap.GetBuffer(0)?;
        let bitmap = self.dc.CreateBitmapFromDxgiSurface(
            &surface,
            Some(&D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            }),
        )?;
        self.dc.SetTarget(&bitmap);
        self.dc.SetDpi(dpi, dpi);
        Ok(())
    }
    pub fn resize(&mut self, width: u32, height: u32, dpi: f32) -> Result<()> {
        unsafe {
            if self.size != (width, height) {
                self.dc.SetTarget(None);
                self.swap.ResizeBuffers(
                    2,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )?;
                self.size = (width, height);
                self.target(dpi)?;
            } else {
                self.dc.SetDpi(dpi, dpi);
            }
            Ok(())
        }
    }
    pub fn begin(&self) {
        unsafe {
            self.dc.BeginDraw();
            self.dc.Clear(Some(&color(0, 0.)));
        }
    }
    pub fn end(&self) -> Result<()> {
        unsafe {
            self.dc.EndDraw(None, None)?;
            self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
            self.composition.Commit()?;
        }
        Ok(())
    }
    pub fn rounded(&self, area: D2D_RECT_F, r: f32, hex: u32, alpha: f32) {
        unsafe {
            self.brush.SetColor(&color(hex, alpha));
            self.dc.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: area,
                    radiusX: r,
                    radiusY: r,
                },
                &self.brush,
            );
        }
    }
    pub fn outline(&self, area: D2D_RECT_F, r: f32, hex: u32, alpha: f32) {
        unsafe {
            self.brush.SetColor(&color(hex, alpha));
            self.dc.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: area,
                    radiusX: r,
                    radiusY: r,
                },
                &self.brush,
                1.,
                None,
            );
        }
    }
    pub fn continuous(&self, area: D2D_RECT_F, r: f32, hex: u32, alpha: f32) -> Result<()> {
        unsafe {
            let (l, t, w, b) = (area.left, area.top, area.right, area.bottom);
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(point(l + r, t), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(point(w - r, t));
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: point(w, t),
                point2: point(w, t),
                point3: point(w, t + r),
            });
            sink.AddLine(point(w, b - r));
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: point(w, b),
                point2: point(w, b),
                point3: point(w - r, b),
            });
            sink.AddLine(point(l + r, b));
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: point(l, b),
                point2: point(l, b),
                point3: point(l, b - r),
            });
            sink.AddLine(point(l, t + r));
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: point(l, t),
                point2: point(l, t),
                point3: point(l + r, t),
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            self.brush.SetColor(&color(hex, alpha));
            self.dc.FillGeometry(&geometry, &self.brush, None);
            Ok(())
        }
    }
    pub fn text(&self, text: &str, area: D2D_RECT_F, hex: u32, alpha: f32, kind: u8) {
        unsafe {
            self.brush.SetColor(&color(hex, alpha));
            let text: Vec<_> = text.encode_utf16().collect();
            self.dc.DrawText(
                &text,
                match kind {
                    1 => &self.small,
                    2 => &self.title,
                    _ => &self.format,
                },
                &area,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    pub fn line(&self, a: (f32, f32), b: (f32, f32), hex: u32, alpha: f32, width: f32) {
        unsafe {
            self.brush.SetColor(&color(hex, alpha));
            self.dc
                .DrawLine(point(a.0, a.1), point(b.0, b.1), &self.brush, width, None);
        }
    }
    pub fn dot(&self, x: f32, y: f32, r: f32, hex: u32, alpha: f32) {
        unsafe {
            self.brush.SetColor(&color(hex, alpha));
            self.dc.FillEllipse(
                &D2D1_ELLIPSE {
                    point: point(x, y),
                    radiusX: r,
                    radiusY: r,
                },
                &self.brush,
            );
        }
    }
    pub fn icon(&self, id: u32, x: f32, y: f32, hex: u32, alpha: f32) {
        let line = |a: (f32, f32), b: (f32, f32)| {
            self.line((x + a.0, y + a.1), (x + b.0, y + b.1), hex, alpha, 1.25)
        };
        match id {
            11 => {
                self.outline(rect(x, y, 14., 10.), 2., hex, alpha);
                line((7., 10.), (7., 14.));
                line((3., 14.), (11., 14.));
            }
            1 => {
                line((1., 3.), (5., 7.));
                line((5., 7.), (1., 11.));
                line((8., 11.), (13., 11.));
            }
            2 => {
                self.outline(rect(x, y, 14., 14.), 7., hex, alpha);
                line((0., 7.), (14., 7.));
                self.outline(rect(x + 4., y, 6., 14.), 3., hex, alpha);
            }
            3 => {
                line((7., 0.), (7., 6.));
                line((2., 4.), (0., 7.));
                line((0., 7.), (2., 12.));
                line((2., 12.), (7., 14.));
                line((7., 14.), (12., 12.));
                line((12., 12.), (14., 7.));
                line((14., 7.), (12., 4.));
            }
            4 => {
                line((6., 1.), (1., 1.));
                line((1., 1.), (1., 13.));
                line((1., 13.), (6., 13.));
                line((5., 7.), (14., 7.));
                line((11., 4.), (14., 7.));
                line((14., 7.), (11., 10.));
            }
            _ => {
                line((2., 7.), (12., 7.));
                line((7., 2.), (12., 7.));
                line((12., 7.), (7., 12.));
            }
        }
    }
}
