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
            GetForegroundWindow, SetCursorPos, ShowCursor,
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

    /// Sets the display count to exactly -1, where winit's own hide leaves it, so its later
    /// `ShowCursor(TRUE)` (the console opening) shows the arrow again. Never lower: Windows can
    /// report the arrow showing while this thread's count is already hidden (a screenshot tool's
    /// overlay, Alt+Tab), and stepping past -1 then left the arrow hidden in menus for good.
    pub fn hide_cursor() {
        for _ in 0..64 {
            // SAFETY: ShowCursor only adjusts this thread's display count.
            let count = unsafe { ShowCursor(0) };
            if count < -1 {
                // SAFETY: as above; undoes the step past -1.
                unsafe { ShowCursor(1) };
            }
            if count < 0 {
                break;
            }
        }
    }

    /// Brings the display count back up to zero, so the arrow shows and winit's next hide (one
    /// step down) hides it again. For when the count ended below where winit left it (a hide
    /// above that raced a screenshot tool or Alt+Tab), which winit's single show cannot undo.
    /// Never above zero, for the same reason as `hide_cursor`.
    pub fn show_cursor() {
        for _ in 0..64 {
            // SAFETY: ShowCursor only adjusts this thread's display count.
            let count = unsafe { ShowCursor(1) };
            if count > 0 {
                // SAFETY: as above; undoes the step past 0.
                unsafe { ShowCursor(0) };
            }
            if count >= 0 {
                break;
            }
        }
    }

    /// The foreground window's client area on screen.
    fn foreground_client() -> Option<RECT> {
        // SAFETY: plain Win32 queries on the foreground window, into local POINT/RECT values.
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
            Some(RECT {
                left: top_left.x,
                top: top_left.y,
                right: bottom_right.x,
                bottom: bottom_right.y,
            })
        }
    }

    /// Whether the cursor is confined inside the foreground window's client area (None when
    /// Windows will not say). winit's lock confines it to the one pixel under it, its
    /// confinement to the whole area; an overlay that frees it leaves the whole desktop.
    pub fn clipped_to_foreground() -> Option<bool> {
        let client = foreground_client()?;
        // SAFETY: GetClipCursor fills a local RECT.
        let clip = unsafe {
            let mut clip: RECT = std::mem::zeroed();
            if GetClipCursor(&mut clip) == 0 {
                return None;
            }
            clip
        };
        Some(
            clip.left >= client.left - 1
                && clip.top >= client.top - 1
                && clip.right <= client.right + 1
                && clip.bottom <= client.bottom + 1,
        )
    }

    /// Puts the cursor in the middle of the foreground window, so a lock taken next lands inside
    /// it (winit locks the cursor where it stands).
    pub fn center_in_foreground() {
        if let Some(client) = foreground_client() {
            // SAFETY: SetCursorPos only moves the cursor.
            unsafe {
                SetCursorPos(
                    (client.left + client.right) / 2,
                    (client.top + client.bottom) / 2,
                );
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn cursor_showing() -> bool {
        false
    }

    pub fn hide_cursor() {}

    pub fn show_cursor() {}

    pub fn clipped_to_foreground() -> Option<bool> {
        None
    }

    pub fn center_in_foreground() {}
}

pub(super) use imp::{
    center_in_foreground, clipped_to_foreground, cursor_showing, hide_cursor, show_cursor,
};
