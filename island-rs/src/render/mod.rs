//! Direct2D + DirectWrite on a DirectComposition swap chain.
//!
//! The window has no redirection bitmap: per-pixel transparency stays on the
//! GPU. All drawing calls take device independent pixels (1/96 inch).

pub mod anim;
pub mod scene;

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::{w, Interface, Result, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// Colour from 0xAARRGGBB.
pub const fn argb(v: u32) -> Color {
    Color {
        r: ((v >> 16) & 0xFF) as f32 / 255.0,
        g: ((v >> 8) & 0xFF) as f32 / 255.0,
        b: (v & 0xFF) as f32 / 255.0,
        a: ((v >> 24) & 0xFF) as f32 / 255.0,
    }
}

pub fn rgb8(r: u8, g: u8, b: u8) -> Color {
    Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
}

impl Color {
    pub fn alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }
    pub fn fade(self, f: f32) -> Color {
        Color { a: self.a * f, ..self }
    }
}

#[derive(Clone, Copy)]
pub enum Paint {
    Solid(Color),
    /// Accent gradient, top left to bottom right of the shape.
    Grad,
    /// Accent gradient, left to right.
    GradH,
}

/// Text style. Sizes are stored in tenths so the style can be a map key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ts {
    size10: u16,
    bold: bool,
    icon: bool,
    ha: u8,
    va: u8,
    trim: bool,
}

pub const fn ts(size: f32) -> Ts {
    Ts { size10: (size * 10.0) as u16, bold: false, icon: false, ha: 0, va: 0, trim: false }
}

impl Ts {
    pub const fn bold(mut self) -> Ts {
        self.bold = true;
        self
    }
    pub const fn icon(mut self) -> Ts {
        self.icon = true;
        self
    }
    pub const fn center(mut self) -> Ts {
        self.ha = 1;
        self
    }
    pub const fn right(mut self) -> Ts {
        self.ha = 2;
        self
    }
    pub const fn vcenter(mut self) -> Ts {
        self.va = 1;
        self
    }
    pub const fn trim(mut self) -> Ts {
        self.trim = true;
        self
    }
    pub fn size(&self) -> f32 {
        self.size10 as f32 / 10.0
    }
}

/// Line height of Segoe UI, as WPF lays it out.
pub fn line_height(size: f32) -> f32 {
    size * 1.33
}

/// Accent presets: name, gradient start, gradient end. The start colour is also the solid accent.
pub const ACCENTS: [(&str, u32, u32); 8] = [
    ("Blau", 0xFF4C9BFF, 0xFF8B6CFF),
    ("Violett", 0xFF9B6CFF, 0xFFD16CFF),
    ("Pink", 0xFFFF5E9C, 0xFFFF8A5E),
    ("Orange", 0xFFFF9F0A, 0xFFFF5E3A),
    ("Grün", 0xFF30D158, 0xFF2FB8A6),
    ("Türkis", 0xFF32C8E6, 0xFF4C7BFF),
    ("Rot", 0xFFFF453A, 0xFFFF2D7A),
    ("Grau", 0xFFC7C7CC, 0xFF7C7C84),
];

static ACCENT_INDEX: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_accent(index: u8) {
    ACCENT_INDEX.store(index.min(ACCENTS.len() as u8 - 1), std::sync::atomic::Ordering::Relaxed);
}

pub fn accent_index() -> u8 {
    ACCENT_INDEX.load(std::sync::atomic::Ordering::Relaxed)
}

/// Solid accent colour of the chosen preset.
pub fn accent() -> Color {
    argb(ACCENTS[accent_index() as usize].1)
}

fn accent_brush(dc: &ID2D1DeviceContext, index: u8) -> Result<ID2D1LinearGradientBrush> {
    let (_, a, b) = ACCENTS[index as usize];
    unsafe {
        let render_target: &ID2D1RenderTarget = dc;
        let stops = render_target.CreateGradientStopCollection(
            &[
                D2D1_GRADIENT_STOP { position: 0.0, color: color_f(argb(a)) },
                D2D1_GRADIENT_STOP { position: 1.0, color: color_f(argb(b)) },
            ],
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        )?;
        dc.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: 0.0 }, endPoint: Vector2 { X: 1.0, Y: 1.0 } },
            None,
            &stops,
        )
    }
}

fn color_f(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

pub struct Gfx {
    _d3d: ID3D11Device,
    dxgi_device: IDXGIDevice,
    swap: IDXGISwapChain1,
    factory: ID2D1Factory1,
    dc: ID2D1DeviceContext,
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    dwrite: IDWriteFactory,

    back: ID2D1Bitmap1,
    /// Only needed during the blur transition: built on demand, dropped by `trim`.
    offscreen: Option<ID2D1Bitmap1>,
    target_size: D2D_SIZE_U,
    shape: ID2D1Bitmap1,
    shape_key: (f32, f32, f32, f32, f32),

    solid: ID2D1SolidColorBrush,
    solid_b: ID2D1Brush,
    grad: ID2D1LinearGradientBrush,
    grad_b: ID2D1Brush,
    /// Preset the gradient brush was built for.
    grad_accent: u8,
    /// Cached radial glow brush and its RGB key, dropped on `trim`.
    radial: Option<(ID2D1RadialGradientBrush, (u8, u8, u8))>,
    /// Cached body mask geometry and `(x, y, w, h, r)` key, dropped on `trim`.
    mask_geo: Option<(ID2D1Geometry, (f32, f32, f32, f32, f32))>,
    shadow: ID2D1Effect,
    blur: ID2D1Effect,
    fillet: ID2D1PathGeometry,
    dash: ID2D1StrokeStyle,
    round: ID2D1StrokeStyle,

    icon_font: PCWSTR,
    formats: HashMap<Ts, IDWriteTextFormat>,
    xf: Vec<Matrix3x2>,

    /// Multiplied into every colour drawn.
    pub alpha: f32,
    /// Present sync interval: 1 waits for the vertical blank, 0 does not
    /// (a second window presents without waiting, so frames are not halved).
    pub sync: u32,
    trimmed_once: bool,
}

impl Gfx {
    pub fn new(hwnd: HWND, width_px: u32, height_px: u32, scale: f32) -> Result<Gfx> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
            let mut created = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            );
            if created.is_err() {
                created = D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut d3d),
                    None,
                    None,
                );
            }
            created?;
            let d3d = d3d.unwrap();
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;

            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width_px,
                Height: height_px,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = dxgi_factory.CreateSwapChainForComposition(&dxgi_device, &desc, None)?;

            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = factory.CreateDevice(&dxgi_device)?;
            let dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dpi = 96.0 * scale;
            dc.SetDpi(dpi, dpi);

            let pixel_format = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
            let surface: IDXGISurface = swap.GetBuffer(0)?;
            let back = dc.CreateBitmapFromDxgiSurface(
                &surface,
                Some(&D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: pixel_format,
                    dpiX: dpi,
                    dpiY: dpi,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    colorContext: ManuallyDrop::new(None),
                }),
            )?;

            let target_props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: pixel_format,
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: ManuallyDrop::new(None),
            };
            let size = D2D_SIZE_U { width: width_px, height: height_px };
            let shape = dc.CreateBitmap(size, None, 0, &target_props)?;

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;

            let solid = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            let grad_accent = accent_index();
            let grad = accent_brush(&dc, grad_accent)?;

            let shadow = dc.CreateEffect(&CLSID_D2D1Shadow)?;
            let blur = dc.CreateEffect(&CLSID_D2D1GaussianBlur)?;

            // Concave corner that blends the island into the screen edge.
            let fillet = factory.CreatePathGeometry()?;
            let sink = fillet.Open()?;
            sink.BeginFigure(Vector2 { X: 8.0, Y: 0.0 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 0.0, Y: 0.0 });
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: Vector2 { X: 8.0, Y: 8.0 },
                size: D2D_SIZE_F { width: 8.0, height: 8.0 },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;

            let dash = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 { dashStyle: D2D1_DASH_STYLE_CUSTOM, ..Default::default() },
                Some(&[4.0, 4.0]),
            )?;
            let round = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    ..Default::default()
                },
                None,
            )?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_font = pick_icon_font(&dwrite);

            Ok(Gfx {
                _d3d: d3d,
                dxgi_device,
                swap,
                factory,
                dc,
                _dcomp: dcomp,
                _target: target,
                _visual: visual,
                dwrite,
                back,
                offscreen: None,
                target_size: size,
                shape,
                shape_key: (0.0, 0.0, 0.0, 0.0, 0.0),
                solid_b: solid.cast()?,
                solid,
                grad_b: grad.cast()?,
                grad,
                grad_accent,
                radial: None,
                mask_geo: None,
                shadow,
                blur,
                fillet: fillet.into(),
                dash: dash.into(),
                round: round.into(),
                icon_font,
                formats: HashMap::new(),
                xf: Vec::new(),
                alpha: 1.0,
                sync: 1,
                trimmed_once: false,
            })
        }
    }

    // ---- Frame ----

    pub fn begin(&mut self) {
        // The accent preset changed in the settings: new gradient brush.
        let index = accent_index();
        if index != self.grad_accent {
            if let Ok(grad) = accent_brush(&self.dc, index) {
                if let Ok(grad_b) = grad.cast() {
                    self.grad = grad;
                    self.grad_b = grad_b;
                    self.grad_accent = index;
                }
            }
        }
        unsafe {
            self.dc.SetTarget(&self.back);
            self.dc.BeginDraw();
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(None);
        }
        self.xf.clear();
        self.alpha = 1.0;
    }

    /// Finishes the frame and waits for the next vertical blank.
    /// False means the device is lost and the caller must rebuild `Gfx`.
    pub fn end(&mut self) -> bool {
        unsafe {
            if self.dc.EndDraw(None, None).is_err() {
                return false;
            }
            self.swap.Present(self.sync, DXGI_PRESENT(0)).is_ok()
        }
    }

    /// Hands cached GPU memory back while the island is idle.
    /// Returns true on the first trim after startup so the caller can touch active render pages.
    pub fn trim(&mut self) -> bool {
        self.offscreen = None;
        self.radial = None;
        self.mask_geo = None;
        unsafe {
            self.dc.SetTarget(None);
            if let Ok(device) = self.dc.GetDevice() {
                device.ClearResources(0);
            }
            if let Ok(dxgi3) = self.dxgi_device.cast::<IDXGIDevice3>() {
                dxgi3.Trim();
            }
            if !self.trimmed_once {
                self.trimmed_once = true;
                let _ = EmptyWorkingSet(GetCurrentProcess());
                return true;
            }
            false
        }
    }

    // ---- Transform stack ----

    fn apply_xf(&self) {
        let m = self.xf.last().copied().unwrap_or(Matrix3x2::identity());
        unsafe { self.dc.SetTransform(&m) };
    }

    fn push_xf(&mut self, m: Matrix3x2) {
        let current = self.xf.last().copied().unwrap_or(Matrix3x2::identity());
        self.xf.push(m * current);
        self.apply_xf();
    }

    pub fn push_translate(&mut self, dx: f32, dy: f32) {
        self.push_xf(Matrix3x2::translation(dx, dy));
    }

    pub fn push_scale(&mut self, cx: f32, cy: f32, sx: f32, sy: f32) {
        self.push_xf(Matrix3x2::scale_around(sx, sy, Vector2 { X: cx, Y: cy }));
    }

    pub fn pop_xf(&mut self) {
        self.xf.pop();
        self.apply_xf();
    }

    // ---- Brushes ----

    fn brush(&mut self, paint: Paint, r: Rect) -> &ID2D1Brush {
        unsafe {
            match paint {
                Paint::Solid(c) => {
                    self.solid.SetColor(&D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * self.alpha });
                    &self.solid_b
                }
                Paint::Grad => {
                    self.grad.SetStartPoint(Vector2 { X: r.x, Y: r.y });
                    self.grad.SetEndPoint(Vector2 { X: r.right(), Y: r.bottom() });
                    self.grad.SetOpacity(self.alpha);
                    &self.grad_b
                }
                Paint::GradH => {
                    self.grad.SetStartPoint(Vector2 { X: r.x, Y: r.cy() });
                    self.grad.SetEndPoint(Vector2 { X: r.right(), Y: r.cy() });
                    self.grad.SetOpacity(self.alpha);
                    &self.grad_b
                }
            }
        }
    }

    // ---- Shapes ----

    pub fn fill_rect(&mut self, r: Rect, paint: Paint) {
        let dc = self.dc.clone();
        let b = self.brush(paint, r);
        unsafe { dc.FillRectangle(&r.d2d(), b) };
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, paint: Paint) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let dc = self.dc.clone();
        let b = self.brush(paint, r);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { dc.FillRoundedRectangle(&rr, b) };
    }

    /// Soft radial glow fading smoothly from `center_color` to transparent over `(rx, ry)`.
    /// Reuses the cached radial brush while the base RGB colour stays the same.
    pub fn fill_radial_glow(&mut self, r: Rect, cx: f32, cy: f32, rx: f32, ry: f32, center_color: Color) {
        let opacity = (center_color.a * self.alpha).clamp(0.0, 1.0);
        if r.w <= 0.0 || r.h <= 0.0 || rx <= 0.0 || ry <= 0.0 || opacity <= 0.002 {
            return;
        }
        let rgb_key = (
            (center_color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (center_color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (center_color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        );
        unsafe {
            let need_new = self.radial.as_ref().map(|(_, k)| *k != rgb_key).unwrap_or(true);
            if need_new {
                let base = rgb8(rgb_key.0, rgb_key.1, rgb_key.2);
                let rt: &ID2D1RenderTarget = &self.dc;
                let Ok(stops) = rt.CreateGradientStopCollection(
                    &[
                        D2D1_GRADIENT_STOP { position: 0.0, color: color_f(base.alpha(1.0)) },
                        D2D1_GRADIENT_STOP { position: 0.55, color: color_f(base.alpha(0.35)) },
                        D2D1_GRADIENT_STOP { position: 1.0, color: color_f(base.alpha(0.0)) },
                    ],
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                ) else {
                    return;
                };
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: Vector2 { X: cx, Y: cy },
                    gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 },
                    radiusX: rx,
                    radiusY: ry,
                };
                let Ok(brush) = self.dc.CreateRadialGradientBrush(&props, None, &stops) else {
                    return;
                };
                self.radial = Some((brush, rgb_key));
            }
            if let Some((brush, _)) = &self.radial {
                brush.SetCenter(Vector2 { X: cx, Y: cy });
                brush.SetRadiusX(rx);
                brush.SetRadiusY(ry);
                brush.SetOpacity(opacity);
                self.dc.FillRectangle(&r.d2d(), brush);
            }
        }
    }

    pub fn stroke_rrect(&mut self, r: Rect, radius: f32, color: Color, width: f32, dashed: bool) {
        let dc = self.dc.clone();
        let style = if dashed { Some(self.dash.clone()) } else { None };
        let b = self.brush(Paint::Solid(color), r);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { dc.DrawRoundedRectangle(&rr, b, width, style.as_ref()) };
    }

    pub fn fill_ellipse(&mut self, cx: f32, cy: f32, radius: f32, paint: Paint) {
        let dc = self.dc.clone();
        let b = self.brush(paint, Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0));
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        unsafe { dc.FillEllipse(&e, b) };
    }

    pub fn stroke_ellipse(&mut self, cx: f32, cy: f32, radius: f32, color: Color, width: f32) {
        let dc = self.dc.clone();
        let b = self.brush(Paint::Solid(color), Rect::default());
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        unsafe { dc.DrawEllipse(&e, b, width, None) };
    }

    /// Arc from 12 o'clock, clockwise, covering `fraction` of the circle.
    pub fn stroke_arc(&mut self, cx: f32, cy: f32, radius: f32, fraction: f32, width: f32, paint: Paint) {
        let fraction = fraction.min(0.9999);
        if fraction <= 0.0001 {
            return;
        }
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = path.Open() else { return };
            let angle = fraction * 2.0 * std::f32::consts::PI;
            sink.BeginFigure(Vector2 { X: cx, Y: cy - radius }, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: Vector2 { X: cx + radius * angle.sin(), Y: cy - radius * angle.cos() },
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_err() {
                return;
            }
            let dc = self.dc.clone();
            let style = self.round.clone();
            let b = self.brush(paint, Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0));
            dc.DrawGeometry(&path, b, width, &style);
        }
    }

    // ---- Text ----

    fn format(&mut self, st: Ts) -> Option<IDWriteTextFormat> {
        if let Some(f) = self.formats.get(&st) {
            return Some(f.clone());
        }
        unsafe {
            let family = if st.icon { self.icon_font } else { w!("Segoe UI") };
            let weight = if st.bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
            let f = self
                .dwrite
                .CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, st.size(), w!("de-DE"))
                .ok()?;
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = f.SetTextAlignment(match st.ha {
                1 => DWRITE_TEXT_ALIGNMENT_CENTER,
                2 => DWRITE_TEXT_ALIGNMENT_TRAILING,
                _ => DWRITE_TEXT_ALIGNMENT_LEADING,
            });
            let _ = f.SetParagraphAlignment(if st.va == 1 {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            });
            if st.trim {
                if let Ok(sign) = self.dwrite.CreateEllipsisTrimmingSign(&f) {
                    let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                    let _ = f.SetTrimming(&trimming, &sign);
                }
            }
            self.formats.insert(st, f.clone());
            Some(f)
        }
    }

    pub fn text(&mut self, s: &str, r: Rect, st: Ts, color: Color) {
        if s.is_empty() || r.w <= 0.0 {
            return;
        }
        let Some(format) = self.format(st) else { return };
        let wide: Vec<u16> = s.encode_utf16().collect();
        let dc = self.dc.clone();
        let options = if st.trim { D2D1_DRAW_TEXT_OPTIONS_CLIP } else { D2D1_DRAW_TEXT_OPTIONS_NONE };
        let b = self.brush(Paint::Solid(color), r);
        unsafe { dc.DrawText(&wide, &format, &r.d2d(), b, options, DWRITE_MEASURING_MODE_NATURAL) };
    }

    /// Width of the text on one line.
    pub fn measure(&mut self, s: &str, st: Ts) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        // Alignment does not change the width; measure with the plain variant.
        let plain = Ts { ha: 0, va: 0, trim: false, ..st };
        let Some(format) = self.format(plain) else { return 0.0 };
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, &format, 10000.0, 1000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return 0.0;
            }
            m.widthIncludingTrailingWhitespace
        }
    }

    // ---- Island base: shadow, body, edge stroke, concave corners ----

    /// Returns the monitor-edge clip rect whenever any side is docked against a screen border.
    pub fn dock_clip(x0: f32, y0: f32, w: f32, h: f32, dock: (f32, f32, f32, f32)) -> Option<Rect> {
        let (dl, dt, dr, db) = dock;
        if dl <= 0.001 && dt <= 0.001 && dr <= 0.001 && db <= 0.001 {
            return None;
        }
        let left = if dl > 0.001 { x0 } else { -1000.0 };
        let top = if dt > 0.001 { y0 } else { -1000.0 };
        let right = if dr > 0.001 { x0 + w } else { 2000.0 };
        let bottom = if db > 0.001 { y0 + h } else { 2000.0 };
        Some(Rect::new(left, top, right - left, bottom - top))
    }

    pub fn island_base(&mut self, x0: f32, y0: f32, w: f32, h: f32, r: f32, dock: (f32, f32, f32, f32)) -> Rect {
        let (dl, dt, dr, db) = dock;
        let body = Rect::new(x0 - r * dl, y0 - r * dt, w + r * (dl + dr), h + r * (dt + db));
        unsafe {
            // The shadow input only changes while the spring or edge-dock blend moves.
            let key = (body.x, body.y, body.w, body.h, r);
            if key != self.shape_key {
                self.shape_key = key;
                self.dc.SetTarget(&self.shape);
                self.dc.Clear(None);
                let saved = self.alpha;
                self.alpha = 1.0;
                self.fill_rrect(body, r, Paint::Solid(argb(0xFF000000)));
                self.alpha = saved;
                self.dc.SetTarget(&self.back);
            }
        }

        let clip = Self::dock_clip(x0, y0, w, h, dock);
        if let Some(c) = clip {
            self.push_clip(c);
        }
        unsafe {
            // WPF DropShadowEffect: BlurRadius 28, ShadowDepth 8 (down), Opacity 0.45.
            let sigma: f32 = 28.0 / 3.0;
            let colour: [f32; 4] = [0.0, 0.0, 0.0, 0.45 * self.alpha];
            let _ = self.shadow.SetValue(D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_ne_bytes());
            let _ = self.shadow.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_VECTOR4, std::slice::from_raw_parts(colour.as_ptr() as *const u8, 16));
            self.shadow.SetInput(0, &self.shape, true);
            if let Ok(image) = self.shadow.GetOutput() {
                self.dc.DrawImage(&image, Some(&Vector2 { X: 0.0, Y: 8.0 }), None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            }
        }

        let black = Paint::Solid(argb(0xFF000000));
        self.fill_rrect(body, r, black);
        self.stroke_rrect(body.inset(0.5), (r - 0.5).max(0.0), argb(0x1AFFFFFF), 1.0, false);
        if clip.is_some() {
            self.pop_clip();
        }

        // Concave corner fillets along docked monitor edges (faded out on corners where two edges meet).
        let fillets = [
            (dt * (1.0 - dl), Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x0 - 7.0, M32: y0 }),
            (dt * (1.0 - dr), Matrix3x2 { M11: -1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x0 + w + 7.0, M32: y0 }),
            (db * (1.0 - dl), Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: -1.0, M31: x0 - 7.0, M32: y0 + h }),
            (db * (1.0 - dr), Matrix3x2 { M11: -1.0, M12: 0.0, M21: 0.0, M22: -1.0, M31: x0 + w + 7.0, M32: y0 + h }),
            (dl * (1.0 - dt), Matrix3x2 { M11: 0.0, M12: 1.0, M21: 1.0, M22: 0.0, M31: x0, M32: y0 - 7.0 }),
            (dl * (1.0 - db), Matrix3x2 { M11: 0.0, M12: -1.0, M21: 1.0, M22: 0.0, M31: x0, M32: y0 + h + 7.0 }),
            (dr * (1.0 - dt), Matrix3x2 { M11: 0.0, M12: 1.0, M21: -1.0, M22: 0.0, M31: x0 + w, M32: y0 - 7.0 }),
            (dr * (1.0 - db), Matrix3x2 { M11: 0.0, M12: -1.0, M21: -1.0, M22: 0.0, M31: x0 + w, M32: y0 + h + 7.0 }),
        ];
        let saved_alpha = self.alpha;
        let dc = self.dc.clone();
        let fillet = self.fillet.clone();
        for (weight, m) in fillets {
            if weight <= 0.01 {
                continue;
            }
            self.alpha = saved_alpha * weight;
            self.push_xf(m);
            let b = self.brush(black, Rect::default());
            unsafe { dc.FillGeometry(&fillet, b, None) };
            self.pop_xf();
        }
        self.alpha = saved_alpha;
        body
    }

    // ---- Layers, clips ----

    pub fn rrect_geometry(&self, r: Rect, radius: f32) -> Option<ID2D1Geometry> {
        unsafe {
            let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
            self.factory.CreateRoundedRectangleGeometry(&rr).ok()?.cast().ok()
        }
    }

    /// Cached rounded-rect geometry for the island body mask; rebuilt only when the shape changes.
    pub fn body_mask(&mut self, r: Rect, radius: f32) -> Option<ID2D1Geometry> {
        let key = (r.x, r.y, r.w, r.h, radius);
        if let Some((geo, k)) = &self.mask_geo {
            if *k == key {
                return Some(geo.clone());
            }
        }
        let geo = self.rrect_geometry(r, radius)?;
        self.mask_geo = Some((geo.clone(), key));
        Some(geo)
    }

    /// Everything until `pop_layer` is clipped to the geometry and faded by `opacity`.
    pub fn push_mask(&mut self, mask: Option<&ID2D1Geometry>, opacity: f32) {
        unsafe {
            let mut params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: D2D_RECT_F { left: -1.0e6, top: -1.0e6, right: 1.0e6, bottom: 1.0e6 },
                geometricMask: ManuallyDrop::new(mask.cloned()),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            self.dc.PushLayer(&params, None);
            ManuallyDrop::drop(&mut params.geometricMask);
        }
    }

    pub fn pop_layer(&mut self) {
        unsafe { self.dc.PopLayer() };
    }

    pub fn push_clip(&mut self, r: Rect) {
        unsafe { self.dc.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn pop_clip(&mut self) {
        unsafe { self.dc.PopAxisAlignedClip() };
    }

    // ---- Offscreen pass for the blur transition ----

    pub fn begin_offscreen(&mut self) {
        if self.offscreen.is_none() {
            let dpi = unsafe {
                let (mut x, mut y) = (0.0, 0.0);
                self.dc.GetDpi(&mut x, &mut y);
                x
            };
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: ManuallyDrop::new(None),
            };
            self.offscreen = unsafe { self.dc.CreateBitmap(self.target_size, None, 0, &props).ok() };
        }
        let Some(offscreen) = &self.offscreen else { return };
        unsafe {
            self.dc.SetTarget(offscreen);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(None);
        }
        self.apply_xf();
    }

    pub fn end_offscreen(&mut self) {
        unsafe { self.dc.SetTarget(&self.back) };
    }

    /// Draws the offscreen pass through a Gaussian blur, with the current transform.
    pub fn draw_offscreen_blurred(&mut self, sigma: f32) {
        unsafe {
            let _ = self.blur.SetValue(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_ne_bytes());
            let Some(offscreen) = &self.offscreen else { return };
            self.blur.SetInput(0, offscreen, true);
            if let Ok(image) = self.blur.GetOutput() {
                self.dc.DrawImage(&image, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            }
        }
    }

    // ---- Bitmaps ----

    /// Pixels are premultiplied BGRA, top row first.
    pub fn create_bitmap(&self, width: u32, height: u32, pixels: &[u8]) -> Option<ID2D1Bitmap1> {
        if pixels.len() < (width * height * 4) as usize {
            return None;
        }
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: ManuallyDrop::new(None),
            };
            self.dc
                .CreateBitmap(D2D_SIZE_U { width, height }, Some(pixels.as_ptr() as *const _), width * 4, &props)
                .ok()
        }
    }

    pub fn draw_bitmap(&mut self, bitmap: &ID2D1Bitmap1, r: Rect) {
        unsafe {
            self.dc.DrawBitmap(bitmap, Some(&r.d2d()), self.alpha, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
        }
    }
}

fn pick_icon_font(dwrite: &IDWriteFactory) -> PCWSTR {
    unsafe {
        let mut collection: Option<IDWriteFontCollection> = None;
        if dwrite.GetSystemFontCollection(&mut collection, false).is_ok() {
            if let Some(collection) = collection {
                let mut index = 0u32;
                let mut exists = windows::core::BOOL(0);
                if collection.FindFamilyName(w!("Segoe Fluent Icons"), &mut index, &mut exists).is_ok() && exists.as_bool() {
                    return w!("Segoe Fluent Icons");
                }
            }
        }
        w!("Segoe MDL2 Assets")
    }
}
