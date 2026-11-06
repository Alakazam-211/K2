//! Screens and windows through Windows Graphics Capture (Windows 10 1903+):
//! the list of monitors and windows, and capture into BGRA frames.

use super::map_error;
use crate::api::{FrameSource, Wake};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{lock, now_us, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use std::sync::{Arc, Mutex};
use windows::core::{IInspectable, Interface, BOOL};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, MONITORINFOF_PRIMARY,
    WS_EX_TOOLWINDOW,
};

// ───────────────────────────── the picker ─────────────────────────────

/// What the system picker chose (a GraphicsCaptureItem).
pub(crate) struct PickedItem(GraphicsCaptureItem);

// SAFETY: GraphicsCaptureItem is an agile WinRT object (usable from any
// thread).
unsafe impl Send for PickedItem {}
// SAFETY: as above.
unsafe impl Sync for PickedItem {}

/// Show the system picker over `opts.parent_window` on a thread of its own
/// (an STA that pumps messages while the picker is open); `done` gets the
/// choice, `NotFound` when cancelled.
pub(crate) fn pick(opts: PickOptions, done: crate::api::Done<Result<ScreenSelection, VideoError>>) {
    let Some(hwnd) = opts.parent_window else {
        return done(Err(VideoError::Invalid("the Windows screen picker needs PickOptions::with_parent_window (the app's HWND)".into())));
    };
    if !supported() {
        return done(Err(unsupported("Windows Graphics Capture needs Windows 10 1903 or later")));
    }
    // `done` runs exactly once: on the thread, or here if it can't start.
    let slot = Arc::new(Mutex::new(Some(done)));
    let s = slot.clone();
    let spawned = std::thread::Builder::new().name("kessel-video-picker".into()).spawn(move || {
        let r = run_picker(hwnd).map(|item| ScreenSelection::Picked(Arc::new(PickedItem(item))));
        if let Some(done) = lock(&s).take() {
            done(r);
        }
    });
    if let Err(e) = spawned {
        if let Some(done) = lock(&slot).take() {
            done(Err(VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}"))));
        }
    }
}

fn run_picker(hwnd: u64) -> Result<GraphicsCaptureItem, VideoError> {
    use windows::Graphics::Capture::GraphicsCapturePicker;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::IInitializeWithWindow;
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT};
    use windows_future::AsyncStatus;

    // SAFETY: this thread's own apartment, for the picker's UI.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let picker = GraphicsCapturePicker::new().map_err(|e| map_error(&e, "GraphicsCapturePicker"))?;
    let init: IInitializeWithWindow = picker.cast().map_err(|e| map_error(&e, "IInitializeWithWindow"))?;
    // SAFETY: the caller's window handle; a stale one fails with an error.
    unsafe { init.Initialize(HWND(hwnd as usize as *mut core::ffi::c_void)) }.map_err(|e| map_error(&e, "IInitializeWithWindow.Initialize"))?;
    let op = picker.PickSingleItemAsync().map_err(|e| map_error(&e, "PickSingleItemAsync"))?;
    loop {
        let mut msg = MSG::default();
        // SAFETY: this thread's message queue; MSG is a plain out struct.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        match op.Status().map_err(|e| map_error(&e, "the picker's status"))? {
            AsyncStatus::Started => {
                // SAFETY: waits for a message or 50 ms; no handles.
                unsafe { MsgWaitForMultipleObjects(None, false, 50, QS_ALLINPUT) };
            }
            // A cancelled pick completes with no item.
            AsyncStatus::Completed => return op.GetResults().map_err(|_| VideoError::NotFound),
            AsyncStatus::Canceled => return Err(VideoError::NotFound),
            _ => return Err(op.ErrorCode().map_or_else(|e| map_error(&e, "the picker"), |c| VideoError::os(c.0 as i64, "the screen picker failed"))),
        }
    }
}

pub(crate) fn supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

struct Monitor {
    handle: HMONITOR,
    rect: RECT,
    primary: bool,
}

fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(m: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: data is the &mut Vec<Monitor> passed below, alive for the
        // whole EnumDisplayMonitors call.
        let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: a monitor handle from the enumeration and a sized struct.
        if unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() {
            out.push(Monitor {
                handle: m,
                rect: info.rcMonitor,
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        BOOL(1)
    }
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only writes to `out` through the LPARAM.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut out as *mut Vec<Monitor> as isize));
    }
    out
}

struct Window {
    hwnd: HWND,
    title: String,
    width: u32,
    height: u32,
}

fn windows_list() -> Vec<Window> {
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: data is the &mut Vec<Window> passed below.
        let out = unsafe { &mut *(data.0 as *mut Vec<Window>) };
        // SAFETY: read-only queries on a window handle from EnumWindows.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
            if pid == std::process::id() {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            let cloak = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut core::ffi::c_void, 4);
            if cloak.is_ok() && cloaked != 0 {
                return BOOL(1);
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return BOOL(1);
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_err() {
                return BOOL(1);
            }
            let (w, h) = ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32);
            if title.trim().is_empty() || w < 64 || h < 64 {
                return BOOL(1);
            }
            out.push(Window { hwnd, title, width: w, height: h });
        }
        BOOL(1)
    }
    let mut out: Vec<Window> = Vec::new();
    // SAFETY: the callback only writes to `out` through the LPARAM.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut Vec<Window> as isize));
    }
    out
}

pub(crate) fn sources() -> Result<Vec<ScreenSourceInfo>, VideoError> {
    if !supported() {
        return Err(unsupported("Windows Graphics Capture needs Windows 10 1903"));
    }
    let mut out = Vec::new();
    for (i, m) in monitors().iter().enumerate() {
        let (w, h) = ((m.rect.right - m.rect.left).max(0) as u32, (m.rect.bottom - m.rect.top).max(0) as u32);
        let title = if m.primary { format!("Display {} (main)", i + 1) } else { format!("Display {}", i + 1) };
        out.push(ScreenSourceInfo::new(format!("monitor:{i}"), ScreenKind::Display, title, w, h));
    }
    for w in windows_list() {
        out.push(ScreenSourceInfo::new(format!("window:{}", w.hwnd.0 as isize), ScreenKind::Window, w.title, w.width, w.height));
    }
    Ok(out)
}

// ───────────────────────────── capture ─────────────────────────────

/// The D3D11 objects a capture uses. The immediate context isn't
/// thread-safe; every use goes through the mutex.
struct Gpu {
    device: ID3D11Device,
    context: Mutex<(ID3D11DeviceContext, Option<Staging>)>,
}

/// The CPU-readable staging texture, with its width and height.
type Staging = (ID3D11Texture2D, u32, u32);

// SAFETY: ID3D11Device is free-threaded; the context and the staging
// texture are only used under the mutex.
unsafe impl Send for Gpu {}
// SAFETY: as above.
unsafe impl Sync for Gpu {}

/// The WinRT device for the frame pool, held by its event handler.
struct AgileDevice(IDirect3DDevice);

// SAFETY: WinRT Direct3D devices are agile (usable from any thread); the
// pool calls its handler on a thread of its own.
unsafe impl Send for AgileDevice {}
// SAFETY: as above.
unsafe impl Sync for AgileDevice {}

fn d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext), VideoError> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: out pointers for the new device and its immediate context.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device as *mut Option<ID3D11Device>),
            None,
            Some(&mut context as *mut Option<ID3D11DeviceContext>),
        )
    }
    .map_err(|e| map_error(&e, "D3D11CreateDevice"))?;
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(VideoError::os(0, "D3D11CreateDevice gave no device")),
    }
}

fn item_for(sel: &ScreenSelection) -> Result<GraphicsCaptureItem, VideoError> {
    let id = match sel {
        ScreenSelection::Listed(id) => id.clone(),
        ScreenSelection::Picked(any) => {
            let picked = any.downcast_ref::<PickedItem>().ok_or(VideoError::NotFound)?;
            return Ok(picked.0.clone());
        }
    };
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| map_error(&e, "IGraphicsCaptureItemInterop"))?;
    if let Some(n) = id.strip_prefix("monitor:").and_then(|n| n.parse::<usize>().ok()) {
        let m = monitors().into_iter().nth(n).ok_or(VideoError::NotFound)?;
        // SAFETY: a live monitor handle from the enumeration.
        unsafe { interop.CreateForMonitor::<GraphicsCaptureItem>(m.handle) }.map_err(|e| map_error(&e, "CreateForMonitor"))
    } else if let Some(h) = id.strip_prefix("window:").and_then(|n| n.parse::<isize>().ok()) {
        let hwnd = HWND(h as *mut core::ffi::c_void);
        // SAFETY: a window handle; a stale one fails with an error.
        unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(hwnd) }.map_err(|_| VideoError::NotFound)
    } else {
        Err(VideoError::NotFound)
    }
}

/// Copy a captured frame's texture to the CPU as BGRA planes.
fn read_frame(gpu: &Gpu, texture: &ID3D11Texture2D) -> Result<(Planes, u32, u32), VideoError> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: a live texture; an out struct.
    unsafe { texture.GetDesc(&mut desc) };
    let (w, h) = (desc.Width, desc.Height);
    let mut guard = lock(&gpu.context);
    let (context, staging) = &mut *guard;
    let fits = matches!(staging, Some((_, sw, sh)) if (*sw, *sh) == (w, h));
    if !fits {
        let sdesc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut t: Option<ID3D11Texture2D> = None;
        // SAFETY: a valid description; an out pointer.
        unsafe { gpu.device.CreateTexture2D(&sdesc, None, Some(&mut t as *mut Option<ID3D11Texture2D>)) }.map_err(|e| map_error(&e, "CreateTexture2D"))?;
        *staging = Some((t.ok_or_else(|| VideoError::os(0, "no staging texture"))?, w, h));
    }
    let Some((st, _, _)) = staging.as_ref() else {
        return Err(VideoError::os(0, "no staging texture"));
    };
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    // SAFETY: same-size textures; the staging copy is mapped for reading
    // while we copy whole rows inside RowPitch x height, then unmapped.
    unsafe {
        context.CopyResource(st, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut mapped as *mut D3D11_MAPPED_SUBRESOURCE)).map_err(|e| map_error(&e, "Map"))?;
        let row = w as usize * 4;
        let ok = !mapped.pData.is_null() && mapped.RowPitch as usize >= row;
        if ok {
            let base = mapped.pData as *const u8;
            for y in 0..h as usize {
                out.extend_from_slice(std::slice::from_raw_parts(base.add(y * mapped.RowPitch as usize), row));
            }
        }
        context.Unmap(st, 0);
        if !ok {
            return Err(VideoError::os(0, format!("a mapped frame with row pitch {}", mapped.RowPitch)));
        }
    }
    Ok((Planes::tight(PixelFormat::Bgra, w, h, out)?, w, h))
}

struct ScreenControl {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    item: GraphicsCaptureItem,
    frame_token: i64,
    closed_token: i64,
}

impl SourceControl for ScreenControl {}

impl Drop for ScreenControl {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.frame_token);
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

pub(crate) fn open(sel: &ScreenSelection, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    if !supported() {
        return Err(unsupported("Windows Graphics Capture needs Windows 10 1903"));
    }
    let item = item_for(sel)?;
    let (device, context) = d3d_device()?;
    let dxgi: IDXGIDevice = device.cast().map_err(|e| map_error(&e, "IDXGIDevice"))?;
    // SAFETY: a live DXGI device.
    let winrt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .and_then(|i| i.cast())
        .map_err(|e| map_error(&e, "CreateDirect3D11DeviceFromDXGIDevice"))?;
    let size = item.Size().map_err(|e| map_error(&e, "GraphicsCaptureItem.Size"))?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)
        .map_err(|e| map_error(&e, "CreateFreeThreaded"))?;
    let session = pool.CreateCaptureSession(&item).map_err(|e| map_error(&e, "CreateCaptureSession"))?;
    let _ = session.SetIsCursorCaptureEnabled(c.cursor);
    // Windows 11 lets us drop the yellow border; older systems refuse.
    let _ = session.SetIsBorderRequired(false);

    let fps = c.fps.unwrap_or(30);
    let shared = SourceShared::new(c, fps, wake);
    let gpu = Arc::new(Gpu {
        device,
        context: Mutex::new((context, None)),
    });
    let last_size = Arc::new(Mutex::new(size));
    let (s, g, d, ls) = (shared.clone(), gpu.clone(), AgileDevice(winrt_device.clone()), last_size.clone());
    let frame_token = pool
        .FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |sender, _| {
            // Capture the whole wrapper (not just its field) so the closure is Send.
            let d = &d;
            let Some(pool) = sender.as_ref() else { return Ok(()) };
            let Ok(frame) = pool.TryGetNextFrame() else { return Ok(()) };
            // The content changed size (a window resized): recreate the pool
            // at the new size; the next frame comes at it.
            if let Ok(cs) = frame.ContentSize() {
                let mut last = lock(&ls);
                if (cs.Width, cs.Height) != (last.Width, last.Height) && cs.Width > 0 && cs.Height > 0 {
                    *last = cs;
                    let _ = pool.Recreate(&d.0, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, SizeInt32 { Width: cs.Width, Height: cs.Height });
                    return Ok(());
                }
            }
            if s.idle() {
                return Ok(());
            }
            let texture = frame
                .Surface()
                .and_then(|surface| surface.cast::<IDirect3DDxgiInterfaceAccess>())
                // SAFETY: the surface's own D3D11 texture.
                .and_then(|access| unsafe { access.GetInterface::<ID3D11Texture2D>() });
            match texture {
                Ok(t) => match read_frame(&g, &t).and_then(|(p, w, h)| VideoFrame::from_planes(p, w, h, now_us())) {
                    Ok(f) => s.deliver(f),
                    Err(e) => s.event(SourceEvent::Error(e)),
                },
                Err(e) => s.event(SourceEvent::Error(map_error(&e, "frame surface"))),
            }
            Ok(())
        }))
        .map_err(|e| map_error(&e, "FrameArrived"))?;
    let s = shared.clone();
    let closed_token = item
        .Closed(&TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
            s.event(SourceEvent::Ended(EndReason::SourceClosed));
            Ok(())
        }))
        .map_err(|e| map_error(&e, "Closed"))?;
    let control = ScreenControl {
        session,
        pool,
        item,
        frame_token,
        closed_token,
    };
    control.session.StartCapture().map_err(|e| map_error(&e, "StartCapture"))?;
    Ok(Box::new(QueuedSource::new(shared, control)))
}
