//! Screen sharing: capture the primary display and downscale frames to JPEG.
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

const INTERVAL: Duration = Duration::from_millis(220);
/// Hard ceiling for one JPEG. Base64 grows it by ~33%; MAX_FRAME is 1 MiB.
const MAX_BYTES: usize = 160_000;

/// Background capture loop. Stop by dropping the value.
pub struct ScreenShare {
    stop: Arc<AtomicBool>,
    pub frames: mpsc::Receiver<Result<Frame>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScreenShare {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, frames) = mpsc::sync_channel(2);
        let thread = std::thread::spawn(move || {
            let mut failures = 0u32;
            while !flag.load(Ordering::Relaxed) {
                let started = Instant::now();
                match capture() {
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
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
fn capture() -> Result<Frame> {
    use windows_sys::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, BitBlt, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
            GetDIBits, CreateCompatibleBitmap, CreateCompatibleDC, ReleaseDC, SelectObject, SRCCOPY,
        },
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };

    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    anyhow::ensure!(width > 0 && height > 0, "Не удалось определить размер экрана");

    let screen = unsafe { GetDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND) };
    anyhow::ensure!(!screen.is_null(), "Нет доступа к экрану");
    let memory = unsafe { CreateCompatibleDC(screen) };
    if memory.is_null() {
        unsafe { ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen) };
        anyhow::bail!("Не удалось создать буфер экрана");
    }
    let bitmap = unsafe { CreateCompatibleBitmap(screen, width, height) };
    if bitmap.is_null() {
        unsafe {
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen);
        }
        anyhow::bail!("Не удалось создать снимок экрана");
    }

    let previous = unsafe { SelectObject(memory, bitmap) };
    let copied = unsafe { BitBlt(memory, 0, 0, width, height, screen, 0, 0, SRCCOPY) };
    // GetDIBits requires the bitmap to be released from the device context.
    unsafe { SelectObject(memory, previous) };

    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        // Negative height keeps rows top-down.
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: 0,
        ..unsafe { std::mem::zeroed() }
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let lines = unsafe {
        GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen);
    }
    anyhow::ensure!(
        copied != 0 && lines == height,
        "Windows не отдал изображение экрана"
    );

    let rgb: Vec<u8> = pixels
        .chunks_exact(4)
        .flat_map(|px| [px[2], px[1], px[0]])
        .collect();
    let image = image::RgbImage::from_raw(width as u32, height as u32, rgb)
        .context("Некорректный размер кадра")?;
    let color = image::DynamicImage::ImageRgb8(image);

    // Try progressively lighter encodings until the frame fits the budget.
    for (target, quality) in [(900u32, 55u8), (800, 48), (640, 40), (480, 32)] {
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
fn capture() -> Result<Frame> {
    anyhow::bail!("Демонстрация экрана поддерживается только в Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "captures the real desktop"]
    fn captures_a_frame() {
        let share = ScreenShare::start();
        let frame = share
            .frames
            .recv_timeout(Duration::from_secs(5))
            .expect("кадр не пришёл")
            .expect("захват экрана не удался");
        assert!(frame.width >= 2 && frame.height >= 2);
        assert!(!frame.data.is_empty() && frame.data.len() <= MAX_BYTES);
        image::load_from_memory(&frame.data).expect("кадр не декодируется");
    }
}
