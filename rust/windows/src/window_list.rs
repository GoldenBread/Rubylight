//! The visible top-level windows with a title and the process each belongs to, for the host to
//! tell a launcher showing a prompt from a game (`butterpollo_session::launcher_prompt`). Read-only:
//! it never sends a message, a click or a key to any of them.

use windows::Win32::{
    Foundation::{HWND, LPARAM},
    Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        WS_EX_TOOLWINDOW,
    },
};

pub struct TopWindow {
    pub pid: u32,
    pub title: String,
    /// Whether it is the window in front.
    pub foreground: bool,
    /// Whether it is minimised, as a game is after an exclusive-fullscreen
    /// mode switch.
    pub minimized: bool,
    /// Whether the window has an owner (`GW_OWNER`), such as a dialog or popup.
    pub owned: bool,
    /// Whether it carries `WS_EX_TOOLWINDOW`.
    pub tool: bool,
    /// Whether DWM reports the window cloaked (`DWMWA_CLOAKED`).
    pub cloaked: bool,
}

/// The visible windows that have a title, on the desktop the signed-in user sees.
pub fn visible() -> Vec<TopWindow> {
    struct Scan {
        front: HWND,
        found: Vec<TopWindow>,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> windows::core::BOOL {
        // SAFETY: EnumWindows passes the `&mut Scan` from `visible` as `data`, which outlives the
        // enumeration and is not otherwise used during it; `text` has the capacity passed.
        unsafe {
            let scan = &mut *(data.0 as *mut Scan);
            if IsWindowVisible(window).as_bool() {
                let length = GetWindowTextLengthW(window);
                if length > 0 {
                    let mut text = vec![0u16; length as usize + 1];
                    let copied = GetWindowTextW(window, &mut text).max(0) as usize;
                    let title = String::from_utf16_lossy(&text[..copied]);
                    if !title.trim().is_empty() {
                        let mut pid = 0;
                        GetWindowThreadProcessId(window, Some(&mut pid));
                        let owned =
                            GetWindow(window, GW_OWNER).is_ok_and(|owner| !owner.is_invalid());
                        let tool = (GetWindowLongPtrW(window, GWL_EXSTYLE) as u32
                            & WS_EX_TOOLWINDOW.0)
                            != 0;
                        let mut cloaked = 0u32;
                        let _ = DwmGetWindowAttribute(
                            window,
                            DWMWA_CLOAKED,
                            (&mut cloaked as *mut u32).cast(),
                            4,
                        );
                        scan.found.push(TopWindow {
                            pid,
                            title,
                            foreground: window == scan.front,
                            minimized: IsIconic(window).as_bool(),
                            owned,
                            tool,
                            cloaked: cloaked != 0,
                        });
                    }
                }
            }
            windows::core::BOOL(1)
        }
    }
    crate::input::follow_input_desktop();
    // SAFETY: GetForegroundWindow has no preconditions.
    let front = unsafe { GetForegroundWindow() };
    let mut scan = Scan {
        front,
        found: vec![],
    };
    // SAFETY: `scan` outlives the synchronous EnumWindows call that reads it through `visit`.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut scan as *mut Scan) as isize));
    }
    scan.found
}
