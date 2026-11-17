//! Screen sharing: capture a monitor or a window and downscale frames to JPEG.
//! Frames travel through the same MLS-encrypted content channel as chat.
use anyhow::{Context, Result};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Encoded frame ready for the session.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// What to capture. `Desktop` covers the whole virtual desktop (all monitors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Desktop,
    Monitor {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Window(isize),
}

/// A selectable capture source shown in the picker.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub title: String,
    pub detail: String,
    pub source: Source,
}

const INTERVAL: Duration = Duration::from_millis(350);
/// Hard ceiling for one JPEG. Base64 grows it by ~33%; the encrypted payload
/// limit is 256 KiB, so this leaves comfortable headroom.
const MAX_BYTES: usize = 40_000;

/// List monitors and top-level windows that can be shared.
pub fn sources() -> Vec<SourceInfo> {
    let mut result = vec![SourceInfo {
        title: "Весь экран".into(),
        detail: "Все мониторы".into(),
        source: Source::Desktop,
    }];
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{BOOL, HWND, LPARAM, RECT},
            Graphics::Gdi::{
                EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
                GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MONITORINFOF_PRIMARY,
                WS_EX_TOOLWINDOW,
            },
        };
        unsafe extern "system" fn collect_monitor(
            monitor: HMONITOR,
            _device: HDC,
            _clip: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            unsafe {
                let list = &mut *(lparam as *mut Vec<(bool, i32, i32, i32, i32, String)>);
                let mut info: MONITORINFOEXW = std::mem::zeroed();
                info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                if GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) == 0 {
                    return 1;
                }
                let rect = info.monitorInfo.rcMonitor;
                let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
                let name = String::from_utf16_lossy(&info.szDevice)
                    .trim_end_matches('\0')
                    .to_owned();
                list.push((
                    primary,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    name,
                ));
                1
            }
        }
        unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
            unsafe {
                let list = &mut *(lparam as *mut Vec<(String, isize, i32, i32)>);
                if IsWindowVisible(hwnd) == 0 {
                    return 1;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == std::process::id() {
                    return 1;
                }
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                if ex_style & WS_EX_TOOLWINDOW != 0 {
                    return 1;
                }
                let length = GetWindowTextLengthW(hwnd);
                if length <= 0 || length > 200 {
                    return 1;
                }
                let mut buffer = vec![0u16; length as usize + 1];
                let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
                if copied <= 0 {
                    return 1;
                }
                let title = String::from_utf16_lossy(&buffer[..copied as usize])
                    .trim()
                    .to_owned();
                if title.is_empty() || title == "Program Manager" {
                    return 1;
                }
                let mut rect: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut rect) == 0 {
                    return 1;
                }
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width < 40 || height < 40 {
                    return 1;
                }
                list.push((title, hwnd as isize, width, height));
                1
            }
        }
        let mut monitors: Vec<(bool, i32, i32, i32, i32, String)> = Vec::new();
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut monitors as *mut _ as LPARAM,
        );
        monitors.sort_by_key(|monitor| (monitor.1, monitor.2));
        if monitors.len() > 1 {
            for (index, (primary, x, y, width, height, name)) in monitors.into_iter().enumerate() {
                let title = if primary {
                    format!("Монитор {} (основной)", index + 1)
                } else {
                    format!("Монитор {}", index + 1)
                };
                result.push(SourceInfo {
                    title,
                    detail: format!("{width}×{height} · {name}"),
                    source: Source::Monitor {
                        x,
                        y,
                        width,
                        height,
                    },
                });
            }
        }
        let mut windows: Vec<(String, isize, i32, i32)> = Vec::new();
        EnumWindows(Some(collect), &mut windows as *mut _ as LPARAM);
        windows.sort_by_key(|(title, _, _, _)| title.to_lowercase());
        for (title, handle, width, height) in windows.into_iter().take(60) {
            result.push(SourceInfo {
                title,
                detail: format!("Окно · {width}×{height}"),
                source: Source::Window(handle),
            });
        }
    }
    result
}

/// Physical rectangle of the monitor that hosts the sharing HUD, plus its DPI
/// scale (1.0 = 96 DPI). Window sharing uses the monitor containing the window.
pub fn hud_geometry(source: Source) -> Option<((i32, i32, i32, i32), f32)> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{HWND, POINT},
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
                MonitorFromPoint, MonitorFromWindow,
            },
            UI::{
                HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
                WindowsAndMessaging::IsWindow,
            },
        };
        let primary = || MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let monitor = match source {
            Source::Desktop => primary(),
            Source::Monitor {
                x,
                y,
                width,
                height,
            } => MonitorFromPoint(
                POINT {
                    x: x + width / 2,
                    y: y + height / 2,
                },
                MONITOR_DEFAULTTONEAREST,
            ),
            Source::Window(handle) => {
                let hwnd = handle as HWND;
                if IsWindow(hwnd) == 0 {
                    primary()
                } else {
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
                }
            }
        };
        if monitor.is_null() {
            return None;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
            dpi_x = 96;
        }
        // Use the work area so the HUD sits above the Windows taskbar.
        let rect = info.rcWork;
        Some((
            (
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ),
            (dpi_x as f32 / 96.0).clamp(0.5, 4.0),
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = source;
        None
    }
}

/// Hide a window from screen capture while keeping it visible on the monitor.
/// Used for our own sharing HUD so viewers do not see it inside the stream.
#[cfg(windows)]
pub fn exclude_from_capture(title: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    unsafe {
        let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        if window.is_null() {
            return false;
        }
        SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE) != 0
    }
}

/// Background capture loop. Stop by dropping the value.
pub struct ScreenShare {
    stop: Arc<AtomicBool>,
    pub frames: mpsc::Receiver<Result<Frame>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScreenShare {
    pub fn start(source: Source) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, frames) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let mut failures = 0u32;
            while !flag.load(Ordering::Relaxed) {
                let started = Instant::now();
                match capture(source) {
                    Ok(frame) => {
                        failures = 0;
                        let _ = tx.try_send(Ok(frame));
                    }
                    Err(error) => {
                        failures += 1;
                        if failures >= 5 {
                            let _ = tx.try_send(Err(error));
                            break;
                        }
                    }
                }
                let elapsed = started.elapsed();
                if elapsed < INTERVAL {
                    let mut left = INTERVAL - elapsed;
                    while left > Duration::from_millis(20) && !flag.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(20));
                        left -= Duration::from_millis(20);
                    }
                    if !flag.load(Ordering::Relaxed) {
                        std::thread::sleep(left);
                    }
                }
            }
        });
        Self {
            stop,
            frames,
            thread: Some(thread),
        }
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // A window capture can block inside GDI for an unresponsive window.
            // Never freeze the UI waiting for it; the thread exits on its own
            // as soon as the current capture returns.
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(windows)]
fn capture(source: Source) -> Result<Frame> {
    let (width, height, pixels) = unsafe { grab(source)? };
    encode(width, height, pixels)
}

#[cfg(windows)]
unsafe fn grab(source: Source) -> Result<(i32, i32, Vec<u8>)> {
    use windows_sys::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetWindowDC, ReleaseDC,
            SRCCOPY, SelectObject,
        },
        Storage::Xps::PrintWindow,
        UI::WindowsAndMessaging::{
            GetSystemMetrics, GetWindowRect, IsWindow, PW_RENDERFULLCONTENT, SM_CXVIRTUALSCREEN,
            SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        },
    };

    unsafe {
        let null = std::ptr::null_mut();
        let (device, release, x, y, width, height) = match source {
            Source::Desktop => {
                let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                if width <= 0 || height <= 0 {
                    anyhow::bail!("Не удалось определить размер экрана");
                }
                let device = GetDC(null);
                (
                    device,
                    null,
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    width,
                    height,
                )
            }
            Source::Monitor {
                x,
                y,
                width,
                height,
            } => (GetDC(null), null, x, y, width, height),
            Source::Window(handle) => {
                let hwnd = handle as HWND;
                if IsWindow(hwnd) == 0 {
                    anyhow::bail!("Окно закрыто");
                }
                let mut rect: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut rect) == 0 {
                    anyhow::bail!("Не удалось получить размер окна");
                }
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                (GetWindowDC(hwnd), hwnd, 0, 0, width, height)
            }
        };
        if device.is_null() {
            anyhow::bail!("Нет доступа к экрану или окну");
        }
        if width <= 0
            || height <= 0
            || width > 8192
            || height > 8192
            || width as i64 * height as i64 > 40_000_000
        {
            ReleaseDC(release, device);
            anyhow::bail!("Слишком большой размер области захвата");
        }

        let memory = CreateCompatibleDC(device);
        if memory.is_null() {
            ReleaseDC(release, device);
            anyhow::bail!("Не удалось создать буфер экрана");
        }
        let bitmap = CreateCompatibleBitmap(device, width, height);
        if bitmap.is_null() {
            DeleteDC(memory);
            ReleaseDC(release, device);
            anyhow::bail!("Не удалось создать снимок экрана");
        }
        let previous = SelectObject(memory, bitmap);
        let copied = match source {
            Source::Desktop | Source::Monitor { .. } => {
                BitBlt(memory, 0, 0, width, height, device, x, y, SRCCOPY) != 0
            }
            Source::Window(handle) => {
                let printed = PrintWindow(handle as HWND, memory, PW_RENDERFULLCONTENT) != 0;
                printed || BitBlt(memory, 0, 0, width, height, device, 0, 0, SRCCOPY) != 0
            }
        };
        // GetDIBits requires the bitmap to be released from the device context.
        SelectObject(memory, previous);

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative height keeps rows top-down.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            ..std::mem::zeroed()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let lines = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        );
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(release, device);
        anyhow::ensure!(
            copied && lines == height,
            "Windows не отдал изображение экрана или окна"
        );
        Ok((width, height, pixels))
    }
}

fn encode(width: i32, height: i32, pixels: Vec<u8>) -> Result<Frame> {
    let rgb: Vec<u8> = pixels
        .chunks_exact(4)
        .flat_map(|px| [px[2], px[1], px[0]])
        .collect();
    let image = image::RgbImage::from_raw(width as u32, height as u32, rgb)
        .context("Некорректный размер кадра")?;
    let color = image::DynamicImage::ImageRgb8(image);

    // Try progressively lighter encodings until the frame fits the budget.
    for (target, quality) in [(800u32, 45u8), (640, 38), (480, 30), (320, 24)] {
        let scale = (target as f32 / width as f32).min(1.0);
        let out_w = ((width as f32 * scale) as u32).max(2);
        let out_h = ((height as f32 * scale) as u32).max(2);
        let resized = color
            .resize_exact(out_w, out_h, image::imageops::FilterType::Triangle)
            .to_rgb8();
        let mut data = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, quality)
            .encode_image(&resized)?;
        if data.len() <= MAX_BYTES {
            return Ok(Frame {
                width: out_w,
                height: out_h,
                data,
            });
        }
    }
    anyhow::bail!("Кадр экрана слишком большой для передачи")
}

#[cfg(not(windows))]
fn capture(_source: Source) -> Result<Frame> {
    anyhow::bail!("Демонстрация экрана поддерживается только в Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "captures the real desktop"]
    fn captures_a_frame() {
        let share = ScreenShare::start(Source::Desktop);
        let frame = share
            .frames
            .recv_timeout(Duration::from_secs(5))
            .expect("кадр не пришёл")
            .expect("захват экрана не удался");
        assert!(frame.width >= 2 && frame.height >= 2);
        assert!(!frame.data.is_empty() && frame.data.len() <= MAX_BYTES);
        image::load_from_memory(&frame.data).expect("кадр не декодируется");
    }

    #[test]
    fn lists_desktop_source() {
        assert!(sources().iter().any(|s| s.source == Source::Desktop));
    }

    #[cfg(windows)]
    #[test]
    fn finds_monitor_for_hud() {
        let (rect, scale) = hud_geometry(Source::Desktop).expect("нет основного монитора");
        assert!(rect.2 > 0 && rect.3 > 0);
        assert!((0.5..=4.0).contains(&scale));
    }

    #[test]
    #[ignore = "captures a real window"]
    fn captures_a_window() {
        let mut last_error = None;
        for info in sources()
            .into_iter()
            .filter(|s| matches!(s.source, Source::Window(_)))
        {
            match capture(info.source) {
                Ok(frame) => {
                    assert!(frame.width >= 2 && frame.height >= 2);
                    assert!(!frame.data.is_empty() && frame.data.len() <= MAX_BYTES);
                    image::load_from_memory(&frame.data).expect("кадр не декодируется");
                    return;
                }
                Err(error) => last_error = Some(error),
            }
        }
        panic!("не удалось захватить ни одно окно: {last_error:?}");
    }
}
