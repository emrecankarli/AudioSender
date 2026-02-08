use std::sync::mpsc::{channel, Receiver, TryRecvError};
use tray_item::TrayItem;

/// Events that can be triggered from the system tray
#[derive(Clone, Debug)]
pub enum TrayEvent {
    Show,
    Quit,
}

/// System tray manager using the tray-item crate with callback-based events
pub struct SystemTray {
    _tray: TrayItem,
    event_receiver: Receiver<TrayEvent>,
}

impl SystemTray {
    /// Create a new system tray icon with "Show" and "Quit" menu items.
    /// Callbacks are executed directly by the OS when clicked, bypassing any sleeping main loop.
    pub fn new() -> anyhow::Result<Self> {
        // Create channel for tray events (for state sync with main app)
        let (tx, rx) = channel::<TrayEvent>();
        let tx_show = tx.clone();
        let tx_quit = tx.clone();
        
        // Create tray icon using Windows resource compiled from app.rc
        let mut tray = TrayItem::new("AudioSender", tray_item::IconSource::Resource("app-icon"))?;
        
        // Add "Show" menu item with callback
        tray.add_menu_item("Show", move || {
            log::info!("Tray: Show clicked - restoring window");
            // Restore window using Win32 API directly in callback
            restore_window();
            // Also notify the main app for state sync
            let _ = tx_show.send(TrayEvent::Show);
        })?;
        
        // Add separator
        tray.inner_mut().add_separator()?;
        
        // Add "Quit" menu item with callback
        tray.add_menu_item("Quit", move || {
            log::info!("Tray: Quit clicked - terminating process");
            let _ = tx_quit.send(TrayEvent::Quit);
            // Exit immediately - this is the key to bypassing the sleeping loop
            std::process::exit(0);
        })?;
        
        log::info!("System tray initialized successfully with tray-item");
        
        Ok(Self {
            _tray: tray,
            event_receiver: rx,
        })
    }
    
    /// Get the next tray event (non-blocking). 
    /// Note: With tray-item, callbacks handle actions directly, so this is mainly for state sync.
    pub fn try_recv(&self) -> Option<TrayEvent> {
        match self.event_receiver.try_recv() {
            Ok(event) => {
                log::debug!("Tray event polled: {:?}", event);
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                log::error!("Tray event channel disconnected!");
                None
            }
        }
    }
}

/// Force the application window to show using Win32 APIs.
/// This is called directly from the tray callback thread, bypassing any sleeping main loop.
fn restore_window() {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, ShowWindow, SetForegroundWindow, SW_RESTORE, SW_SHOW,
    };
    use windows::core::PCWSTR;

    unsafe {
        // Look for the window by title. Title must match exactly as set in run_native
        let title: Vec<u16> = "AudioSender\0".encode_utf16().collect();
        let hwnd = FindWindowW(None, PCWSTR(title.as_ptr()));
        
        // Check if window was found (HWND is not null)
        if hwnd.0 != 0 {
            log::info!("Win32: Found window, forcing visibility");
            // SW_RESTORE handles both minimized and hidden states
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        } else {
            log::warn!("Win32: Could not find window 'AudioSender'");
        }
    }
}

/// Generate a simple orange circle icon as RGBA bytes
fn generate_icon_ico() -> Vec<u8> {
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];
    
    let center = size / 2;
    let radius = (size / 2 - 2) as i32;
    
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let dx = x as i32 - center as i32;
            let dy = y as i32 - center as i32;
            let dist_sq = dx * dx + dy * dy;
            
            if dist_sq <= radius * radius {
                // Inside circle - orange color
                rgba[idx] = 255;     // R
                rgba[idx + 1] = 140; // G
                rgba[idx + 2] = 0;   // B
                rgba[idx + 3] = 255; // A
            } else {
                // Outside - transparent
                rgba[idx + 3] = 0;
            }
        }
    }
    
    rgba
}
