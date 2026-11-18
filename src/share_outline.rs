//! Local, click-through indication of the physical region being captured.
#![cfg(windows)]

use std::{ptr::null_mut, sync::OnceLock};
use thodtalk::share::{Source, SourceInfo};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
        PAINTSTRUCT, RGN_DIFF, SetWindowRgn,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, HTTRANSPARENT, HWND_TOPMOST,
        IsIconic, IsWindowVisible, LWA_ALPHA, RegisterClassW, SW_HIDE, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos,
        ShowWindow, WDA_EXCLUDEFROMCAPTURE, WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    },
};

type Bounds = (i32, i32, i32, i32);

pub struct ShareOutline {
    borders: Vec<Border>,
    window_source: Option<HWND>,
}

impl ShareOutline {
    pub fn new(source: Source, sources: &[SourceInfo]) -> Self {
        let mut outline = Self {
            borders: Vec::new(),
            window_source: None,
        };
        match source {
            Source::Desktop => {
                for item in sources {
                    if let Source::Monitor {
                        x,
                        y,
                        width,
                        height,
                    } = item.source
                    {
                        outline.add(Some((x, y, width, height)));
                    }
                }
            }
            Source::Monitor {
                x,
                y,
                width,
                height,
            } => outline.add(Some((x, y, width, height))),
            Source::Window(handle) => {
                let window = handle as HWND;
                outline.window_source = Some(window);
                outline.add(window_bounds(window));
            }
        }
        outline
    }

    fn add(&mut self, bounds: Option<Bounds>) {
        if let Some(mut border) = Border::new() {
            border.place(bounds);
            self.borders.push(border);
        }
    }

    pub fn update(&mut self) {
        if let (Some(window), Some(border)) = (self.window_source, self.borders.first_mut()) {
            border.place(window_bounds(window));
        }
    }
}

fn window_bounds(window: HWND) -> Option<Bounds> {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if IsWindowVisible(window) == 0
            || IsIconic(window) != 0
            || GetWindowRect(window, &mut rect) == 0
        {
            return None;
        }
        Some((
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        ))
    }
}

struct Border {
    window: HWND,
    bounds: Option<Bounds>,
}

impl Border {
    fn new() -> Option<Self> {
        static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
        let class = CLASS.get_or_init(|| {
            let name: Vec<_> = "ThodTalkShareOutline"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            unsafe {
                let mut class: WNDCLASSW = std::mem::zeroed();
                class.hInstance = GetModuleHandleW(std::ptr::null());
                class.lpszClassName = name.as_ptr();
                class.lpfnWndProc = Some(paint_outline);
                RegisterClassW(&class);
            }
            name
        });
        unsafe {
            let window = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            if window.is_null() {
                return None;
            }
            // Layered overlays are skipped by the desktop's SRCCOPY capture.
            SetLayeredWindowAttributes(window, 0, 255, LWA_ALPHA);
            SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
            Some(Self {
                window,
                bounds: None,
            })
        }
    }

    fn place(&mut self, bounds: Option<Bounds>) {
        let bounds = bounds.filter(|(_, _, width, height)| *width > 6 && *height > 6);
        if self.bounds == bounds {
            return;
        }
        unsafe {
            let Some((x, y, width, height)) = bounds else {
                ShowWindow(self.window, SW_HIDE);
                self.bounds = None;
                return;
            };
            if self
                .bounds
                .is_none_or(|(_, _, w, h)| w != width || h != height)
            {
                let outer = CreateRectRgn(0, 0, width, height);
                let inner = CreateRectRgn(3, 3, width - 3, height - 3);
                let region_valid = !outer.is_null()
                    && !inner.is_null()
                    && CombineRgn(outer, outer, inner, RGN_DIFF) != 0;
                DeleteObject(inner);
                if !region_valid || SetWindowRgn(self.window, outer, 1) == 0 {
                    DeleteObject(outer);
                    return;
                }
            }
            if SetWindowPos(
                self.window,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            ) != 0
            {
                self.bounds = bounds;
            }
        }
    }
}

impl Drop for Border {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.window);
        }
    }
}

unsafe extern "system" fn paint_outline(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            WM_PAINT => {
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(window, &mut paint);
                let orange = CreateSolidBrush(0x00398cff);
                FillRect(dc, &paint.rcPaint, orange);
                DeleteObject(orange);
                EndPaint(window, &paint);
                0
            }
            _ => DefWindowProcW(window, message, w, l),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::{GetWindowRgn, PtInRegion};

    #[test]
    #[ignore = "shows a small click-through outline on the local desktop"]
    fn creates_a_thin_hollow_capture_outline() {
        let mut border = Border::new().expect("Windows не создала окно рамки");
        let bounds = (10, 10, 100, 50);
        border.place(Some(bounds));
        assert_eq!(border.bounds, Some(bounds));
        unsafe {
            assert_ne!(IsWindowVisible(border.window), 0);
            let region = CreateRectRgn(0, 0, 0, 0);
            assert!(!region.is_null());
            let shape = GetWindowRgn(border.window, region);
            let edge = PtInRegion(region, 1, 20);
            let inside = PtInRegion(region, 5, 20);
            DeleteObject(region);
            assert!(shape > 1 && edge != 0 && inside == 0);
        }
        border.place(None);
        assert_eq!(unsafe { IsWindowVisible(border.window) }, 0);
    }
}
