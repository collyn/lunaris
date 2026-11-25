#![cfg_attr(not(target_os = "windows"), allow(dead_code))]
//! Whether a window is being dragged on a Windows host while the left
//! button is held.
//!
//! Windows draws its pointer into the picture while a window is moved. The
//! system's own move loop says so (`GUI_INMOVESIZE`, lunaris-media
//! `in_window_move_size`), but the many apps that move their windows
//! themselves (their own title bars, a browser tab torn off) do not. Viewers
//! used to look for the pointer in the picture on every drag of a Windows
//! host, and each look copies a frame back from the GPU: selecting an area
//! made the picture skip frames (2026-11-25). Here the host watches instead:
//! the top-level window under the pointer when the button went down, and
//! whether it moved or changed size since. Viewers that see
//! `window_drag_watched` in `host_cursor` trust `in_window_move_size` and no
//! longer look (web/src/pointerInPicture.ts `looksForPointerInPicture`).

/// A window's place on the screen (left, top, right, bottom).
pub type Rect = (i32, i32, i32, i32);

/// The decision over one press of the left button.
#[derive(Default)]
pub struct WindowDrag {
    held: bool,
    /// The window under the pointer when the button went down, and its place then.
    start: Option<Rect>,
    moved: bool,
}

impl WindowDrag {
    /// One look: whether the left button is held now, and the place of the
    /// window under the pointer when it went down (`rect_at_press` is asked
    /// only at the press) and now (`rect_now`, for that same window). Returns
    /// whether a window is being dragged.
    pub fn update(
        &mut self,
        held: bool,
        rect_at_press: impl FnOnce() -> Option<Rect>,
        rect_now: impl FnOnce() -> Option<Rect>,
    ) -> bool {
        if !held {
            *self = Self::default();
            return false;
        }
        if !self.held {
            self.held = true;
            self.start = rect_at_press();
            return false;
        }
        if let (false, Some(start)) = (self.moved, self.start) {
            if let Some(now) = rect_now() {
                self.moved = start != now;
            }
        }
        self.moved
    }
}

#[cfg(target_os = "windows")]
mod system {
    use super::{Rect, WindowDrag};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetCursorPos, GetDesktopWindow, GetShellWindow, GetWindowRect, WindowFromPoint,
        GA_ROOT,
    };

    fn rect_of(hwnd: HWND) -> Option<Rect> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut r).ok()? };
        Some((r.left, r.top, r.right, r.bottom))
    }

    /// Watches the window under the pointer at each press.
    #[derive(Default)]
    pub struct Watch {
        drag: WindowDrag,
        window: Option<isize>,
    }

    impl Watch {
        /// Whether a window is being dragged now.
        pub fn poll(&mut self) -> bool {
            // The high bit: the button is down now (the physical mouse's or
            // one the viewer's input pressed).
            let held = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
            // Read before the press may set it: `rect_now` is not asked then.
            let current = self.window;
            let window = &mut self.window;
            self.drag.update(
                held,
                || {
                    // Screen coordinates, whatever display is streamed.
                    let mut at = POINT::default();
                    if unsafe { GetCursorPos(&mut at) }.is_err() {
                        *window = None;
                        return None;
                    }
                    let root = unsafe { GetAncestor(WindowFromPoint(at), GA_ROOT) };
                    // The desktop itself (selecting icons) never moves.
                    let desktop = unsafe { [GetDesktopWindow(), GetShellWindow()] };
                    if root.0.is_null() || desktop.contains(&root) {
                        *window = None;
                        return None;
                    }
                    *window = Some(root.0 as isize);
                    rect_of(root)
                },
                move || current.and_then(|w| rect_of(HWND(w as *mut _))),
            )
        }
    }
}

#[cfg(target_os = "windows")]
pub use system::Watch;

#[cfg(test)]
mod tests {
    use super::*;

    const A: Rect = (100, 100, 900, 700);
    const MOVED: Rect = (140, 120, 940, 720);

    #[test]
    fn a_window_that_moves_under_a_held_button_is_dragged() {
        let mut d = WindowDrag::default();
        assert!(!d.update(true, || Some(A), || unreachable!("not asked at the press")));
        assert!(!d.update(true, || unreachable!(), || Some(A)), "held still");
        assert!(d.update(true, || unreachable!(), || Some(MOVED)));
        // Stays a drag until the button is let go, also once the window stops.
        assert!(d.update(true, || unreachable!(), || Some(MOVED)));
        assert!(!d.update(false, || unreachable!(), || unreachable!()));
    }

    #[test]
    fn selecting_an_area_drags_no_window() {
        let mut d = WindowDrag::default();
        // On the desktop: no window to watch.
        d.update(true, || None, || unreachable!());
        for _ in 0..50 {
            assert!(!d.update(true, || unreachable!(), || unreachable!()));
        }
        d.update(false, || None, || None);
        // In a window that stays where it is.
        d.update(true, || Some(A), || unreachable!());
        for _ in 0..50 {
            assert!(!d.update(true, || unreachable!(), || Some(A)));
        }
    }

    #[test]
    fn a_resized_window_counts_and_each_press_starts_afresh() {
        let mut d = WindowDrag::default();
        d.update(true, || Some(A), || None);
        assert!(d.update(true, || unreachable!(), || Some((100, 100, 1000, 700))));
        d.update(false, || None, || None);
        d.update(true, || Some(MOVED), || None);
        assert!(!d.update(true, || unreachable!(), || Some(MOVED)), "the new place is the start");
        // A window gone (closed while held): nothing to say.
        assert!(!d.update(true, || unreachable!(), || None));
    }
}
