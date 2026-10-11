//! What Windows itself says about the cursor, for the times the game's view of it goes stale.
//!
//! winit hides the cursor through Windows' `ShowCursor` count and confines it with `ClipCursor`,
//! and remembers what it did, so it does not do either again while it thinks they hold. An
//! overlay that opens over the game (the NVIDIA overlay on Alt+Z) shows and frees the cursor
//! while it is up and does not always put it back: the game kept its mouse look, but the arrow
//! stayed on screen and could wander off the window. These read the OS's state so the cursor
//! code can put it back.

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{
            CURSOR_SHOWING, CURSORINFO, GetClientRect, GetClipCursor, GetCursorInfo,
            GetForegroundWindow, ShowCursor,
        },
    };

    /// The arrow is drawn (Windows' display count is at or above zero).
    pub fn cursor_showing() -> bool {
        // SAFETY: CURSORINFO is plain data, filled by GetCursorInfo once cbSize is set.
        unsafe {
            let mut info: CURSORINFO = std::mem::zeroed();
            info.cbSize = size_of::<CURSORINFO>() as u32;
            GetCursorInfo(&mut info) != 0 && info.flags & CURSOR_SHOWING != 0
        }
    }

    /// Brings the display count down to -1, where winit's own hide leaves it, so its later
    /// `ShowCursor(TRUE)` (the console opening) shows the arrow again.
    pub fn hide_cursor() {
        for _ in 0..64 {
            // SAFETY: ShowCursor only adjusts this thread's display count.
            if unsafe { ShowCursor(0) } < 0 {
                break;
            }
        }
    }

    /// Whether the cursor is confined to the foreground window's client area (None when
    /// Windows will not say).
    pub fn clipped_to_foreground() -> Option<bool> {
        // SAFETY: plain Win32 queries on the foreground window and the clip rect, into local
        // POINT/RECT values.
        unsafe {
            let window = GetForegroundWindow();
            if window.is_null() {
                return None;
            }
            let mut client: RECT = std::mem::zeroed();
            if GetClientRect(window, &mut client) == 0 {
                return None;
            }
            let mut top_left = POINT {
                x: client.left,
                y: client.top,
            };
            let mut bottom_right = POINT {
                x: client.right,
                y: client.bottom,
            };
            if ClientToScreen(window, &mut top_left) == 0
                || ClientToScreen(window, &mut bottom_right) == 0
            {
                return None;
            }
            let mut clip: RECT = std::mem::zeroed();
            if GetClipCursor(&mut clip) == 0 {
                return None;
            }
            let near = |a: i32, b: i32| (a - b).abs() <= 1;
            Some(
                near(clip.left, top_left.x)
                    && near(clip.top, top_left.y)
                    && near(clip.right, bottom_right.x)
                    && near(clip.bottom, bottom_right.y),
            )
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn cursor_showing() -> bool {
        false
    }

    pub fn hide_cursor() {}

    pub fn clipped_to_foreground() -> Option<bool> {
        None
    }
}

pub(super) use imp::{clipped_to_foreground, cursor_showing, hide_cursor};
