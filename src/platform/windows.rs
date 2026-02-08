use std::sync::atomic::{AtomicBool, Ordering};

static CONSOLE_ATTACHED: AtomicBool = AtomicBool::new(true);

/// Windows-specific console window management
#[cfg(windows)]
pub mod console {
    use super::*;
    use windows::Win32::System::Console::{FreeConsole, GetConsoleWindow, AllocConsole};
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOW};
    use windows::Win32::Foundation::HWND;

    /// Check if HWND is valid
    fn is_valid_hwnd(hwnd: HWND) -> bool {
        hwnd.0 != 0
    }

    /// Completely detach/free the console window (irreversible until restart)
    pub fn free_console() {
        unsafe {
            // FreeConsole completely detaches the console from this process
            // This is permanent until the process exits
            let result = FreeConsole();
            if result.is_ok() {
                CONSOLE_ATTACHED.store(false, Ordering::SeqCst);
                log::info!("Console freed (detached from process)");
            } else {
                log::debug!("FreeConsole failed - console may already be detached");
            }
        }
    }

    /// Hide the console window (can be shown again with show_console)
    pub fn hide_console() {
        unsafe {
            let hwnd = GetConsoleWindow();
            if is_valid_hwnd(hwnd) {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    /// Show the console window
    pub fn show_console() {
        unsafe {
            let hwnd = GetConsoleWindow();
            if is_valid_hwnd(hwnd) {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
        }
    }

    /// Check if console is still attached to this process
    pub fn is_console_attached() -> bool {
        CONSOLE_ATTACHED.load(Ordering::SeqCst)
    }

    /// Set console visibility at startup based on preference
    /// If false: completely removes the console (requires restart to bring back)
    /// If true: keeps console visible
    pub fn setup_console_at_startup(show_console: bool) {
        if !show_console {
            // Hide initially, then free completely
            hide_console();
            // Give a moment for any initial logs to be written
            std::thread::sleep(std::time::Duration::from_millis(100));
            free_console();
        }
        // If show_console is true, do nothing - console is already visible
    }
}

#[cfg(not(windows))]
pub mod console {
    pub fn free_console() {}
    pub fn hide_console() {}
    pub fn show_console() {}
    pub fn is_console_attached() -> bool { true }
    pub fn setup_console_at_startup(_show_console: bool) {}
}
