use eframe::egui;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::capture::AudioCapture;
use crate::config::{Config, startup};
use crate::network::discovery::DiscoveryService;
use crate::network::streaming::StreamingClient;
use crate::network::protocol::AudioPacket;
use crate::network::socket::create_listener_socket;
use crate::audio::types::AudioFrame;
use crate::audio::codec::{OpusEncoder, OpusDecoder};
use crate::audio::playback::AudioPlayback;
use crate::utils::{ensure_port_free, kill_existing_instances};

use super::state::SharedState;
use super::tray::{SystemTray, TrayEvent};

/// Application mode
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum AppMode {
    Server,
    Client,
}

/// Background worker command
#[derive(Clone, Debug)]
pub enum WorkerCommand {
    StartServer { device: Option<String> },
    StartClient { server_ip: Option<String> },
    Stop,
}

/// Background worker response
#[derive(Clone, Debug)]
pub enum WorkerResponse {
    Started,
    Stopped,
    Error(String),
    Stats { packets: u32, latency: u32 },
    ConnectedToServer { ip: String },  // Reports successful connection for saving
}

/// Main application struct
pub struct AudioSenderApp {
    // Mode
    mode: AppMode,
    
    // Device selection
    devices: Vec<String>,
    selected_device: usize,
    
    // Client settings
    server_ip: String,
    
    // Volume (0.0 - 1.0)
    volume: f32,
    
    // Shared state with background threads
    shared_state: SharedState,
    
    // Worker thread communication
    worker_tx: Option<Sender<WorkerCommand>>,
    worker_rx: Option<Receiver<WorkerResponse>>,
    worker_handle: Option<JoinHandle<()>>,
    
    // System tray
    tray: Option<SystemTray>,
    tray_tx: Option<Sender<TrayEvent>>,
    
    // Window state
    show_window: bool,
    window_visible: bool,
    start_minimized: bool,
    
    // Settings window
    show_settings: bool,
    config: Config,
    
    // Startup setting changed (needs registry update)
    startup_changed: bool,
    
    // Saved window position/size for persistence
    save_window_state: bool,
    
    // Auto-start on first frame
    auto_start_pending: bool,
}

impl AudioSenderApp {
    fn from_config(config: Config, start_minimized: bool) -> Self {
        let devices = AudioCapture::list_devices().unwrap_or_default();
        
        // Load last used settings from config
        let mode = config.get_last_mode();
        let selected_device = config.last_device_index.min(devices.len().saturating_sub(1));
        let server_ip = config.last_server_ip.clone();
        let volume = config.last_volume.clamp(0.0, 1.0);
        
        Self {
            mode,
            devices,
            selected_device,
            server_ip,
            volume,
            shared_state: SharedState::new(),
            worker_tx: None,
            worker_rx: None,
            worker_handle: None,
            tray: None,
            tray_tx: None,
            show_window: true,
            window_visible: !start_minimized,
            start_minimized,
            show_settings: false,
            config: config.clone(),
            startup_changed: false,
            save_window_state: true,
            auto_start_pending: config.auto_start_server || config.auto_start_client,
        }
    }
}

impl AudioSenderApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: Config, start_minimized: bool) -> Self {
        // Set dark mode by default
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        
        let mut app = Self::from_config(config, start_minimized);
        
        // Initialize system tray with tray-item (callback-based, bypasses sleeping loop)
        match SystemTray::new() {
            Ok(tray) => {
                app.tray = Some(tray);
                log::info!("System tray initialized with tray-item");
            }
            Err(e) => {
                log::warn!("Failed to create system tray: {}", e);
            }
        }
        
        // Note: Window position restoration is platform-specific
        // and not directly supported in egui's ViewportCommand
        
        app
    }
    
    /// Save all current settings to config
    fn save_settings(&mut self) {
        // Update config with current values
        self.config.set_last_mode(self.mode);
        self.config.last_device_index = self.selected_device;
        self.config.last_server_ip = self.server_ip.clone();
        self.config.last_volume = self.volume;
        
        // Save to file
        if let Err(e) = self.config.save() {
            log::error!("Failed to save config: {}", e);
        } else {
            log::info!("Settings saved");
        }
    }
    
    /// Start the background worker thread
    fn start_worker(&mut self) {
        let (cmd_tx, cmd_rx) = channel::<WorkerCommand>();
        let (resp_tx, resp_rx) = channel::<WorkerResponse>();
        
        self.worker_tx = Some(cmd_tx);
        self.worker_rx = Some(resp_rx);
        
        let shared_state = self.shared_state.clone();
        
        let handle = thread::spawn(move || {
            worker_loop(cmd_rx, resp_tx, shared_state);
        });
        
        self.worker_handle = Some(handle);
    }
    
    /// Send command to worker
    fn send_command(&self, cmd: WorkerCommand) {
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(cmd);
        }
    }
    
    /// Check for worker responses
    fn check_worker_responses(&mut self) {
        if let Some(ref rx) = self.worker_rx {
            while let Ok(resp) = rx.try_recv() {
                match resp {
                    WorkerResponse::Started => {
                        self.shared_state.set_connected(true);
                    }
                    WorkerResponse::Stopped => {
                        self.shared_state.set_connected(false);
                    }
                    WorkerResponse::Error(msg) => {
                        self.shared_state.set_error(Some(msg));
                        self.shared_state.set_connected(false);
                    }
                    WorkerResponse::Stats { packets, latency } => {
                        self.shared_state.set_packets_per_sec(packets);
                        self.shared_state.set_latency(latency);
                    }
                    WorkerResponse::ConnectedToServer { ip } => {
                        // Save the successfully connected server IP
                        if self.config.last_server_ip != ip {
                            log::info!("Saving successfully connected server IP: {}", ip);
                            self.config.last_server_ip = ip;
                            self.server_ip = self.config.last_server_ip.clone();
                            let _ = self.config.save();
                        }
                    }
                }
            }
        }
    }
    
    /// Check for tray events - this should always run even when window is hidden
    fn check_tray_events(&mut self, ctx: &egui::Context) {
        // Collect all pending tray events first (to avoid borrow issues)
        let events: Vec<TrayEvent> = if let Some(ref tray) = self.tray {
            let mut events = Vec::new();
            while let Some(event) = tray.try_recv() {
                events.push(event);
            }
            events
        } else {
            return;
        };
        
        // Process collected events (mainly for state sync, callbacks already handle actions)
        for event in events {
            log::info!("Tray event received: {:?}", event);
            match event {
                TrayEvent::Show => {
                    log::info!("Tray: Show window requested - syncing state");
                    self.show_window = true;
                    self.window_visible = true;
                    // Window already restored by callback, but sync state
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayEvent::Quit => {
                    log::info!("Tray: Quit requested - closing application");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
    
    /// Start streaming based on current mode
    fn start_streaming(&mut self) {
        if self.shared_state.is_connected() {
            return;
        }
        
        // Save settings before starting
        self.save_settings();
        
        // Clear any previous error
        self.shared_state.clear_error();
        self.shared_state.set_status("Starting...");
        
        // Start worker if not already running
        if self.worker_handle.is_none() {
            self.start_worker();
        }
        
        match self.mode {
            AppMode::Server => {
                let device = self.devices.get(self.selected_device).cloned();
                self.shared_state.set_status("Starting Server...");
                self.send_command(WorkerCommand::StartServer { device });
            }
            AppMode::Client => {
                let ip = if self.server_ip.is_empty() {
                    None
                } else {
                    Some(self.server_ip.clone())
                };
                self.shared_state.set_status("Connecting...");
                self.send_command(WorkerCommand::StartClient { server_ip: ip });
            }
        }
    }
    
    /// Stop streaming
    fn stop_streaming(&mut self) {
        self.shared_state.set_status("Stopping...");
        self.send_command(WorkerCommand::Stop);
    }
    
    /// Get display status
    fn get_status_display(&self) -> (String, egui::Color32) {
        if let Some(ref err) = self.shared_state.get_error() {
            return (format!("Error: {}", err), egui::Color32::RED);
        }
        
        if self.shared_state.is_connected() {
            match self.mode {
                AppMode::Server => ("Streaming (Server)".to_string(), egui::Color32::GREEN),
                AppMode::Client => ("Connected (Client)".to_string(), egui::Color32::GREEN),
            }
        } else {
            (self.shared_state.get_status(), egui::Color32::GRAY)
        }
    }
    
    /// Update startup registry if setting changed
    fn apply_startup_setting(&mut self) {
        if !self.startup_changed {
            return;
        }
        
        match startup::toggle_startup(self.config.start_with_windows) {
            Ok(()) => {
                log::info!("Startup setting updated: {}", self.config.start_with_windows);
                self.startup_changed = false;
            }
            Err(e) => {
                log::error!("Failed to update startup setting: {}", e);
                self.shared_state.set_error(Some(format!("Failed to update startup: {}", e)));
            }
        }
    }
}

impl eframe::App for AudioSenderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Always request repaint to keep the app responsive
        ctx.request_repaint_after(Duration::from_millis(50));
        
        // Always check for worker responses - even when hidden
        self.check_worker_responses();
        
        // Always check for tray events - this is critical for tray to work
        self.check_tray_events(ctx);
        
        // Handle auto-start on first frame
        if self.auto_start_pending {
            self.auto_start_pending = false;
            
            // Auto-start server if configured and in server mode
            if self.config.auto_start_server && self.mode == AppMode::Server {
                log::info!("Auto-starting server on launch...");
                self.start_streaming();
            }
            // Auto-start client if configured and in client mode with saved IP
            else if self.config.auto_start_client && self.mode == AppMode::Client {
                if !self.server_ip.is_empty() {
                    log::info!("Auto-starting client on launch, connecting to: {}", self.server_ip);
                    self.start_streaming();
                } else {
                    log::warn!("Auto-start client enabled but no server IP saved");
                    self.shared_state.set_error(Some(
                        "Auto-start client enabled but no server IP saved. Please enter an IP and connect once to save it.".to_string()
                    ));
                }
            }
        }
        
        // Handle minimize to tray on startup
        if self.start_minimized {
            self.start_minimized = false;
            self.window_visible = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        
        // Main UI
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                
                // Header
                ui.horizontal(|ui| {
                    ui.heading("🔊 AudioSender");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                    });
                });
                ui.add_space(10.0);
                
                // Mode Selection Section
                egui::Frame::none()
                    .fill(ui.style().visuals.extreme_bg_color)
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!self.shared_state.is_connected(), |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Mode:");
                                ui.add_space(10.0);
                                if ui.radio_value(&mut self.mode, AppMode::Server, "📡 Server").clicked() {
                                    self.save_settings();
                                }
                                ui.add_space(5.0);
                                if ui.radio_value(&mut self.mode, AppMode::Client, "🎧 Client").clicked() {
                                    self.save_settings();
                                }
                            });
                        });
                });
                ui.add_space(10.0);
                
                // Configuration Section
                egui::Frame::none()
                    .fill(ui.style().visuals.extreme_bg_color)
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        // Server mode: Device selection
                        if self.mode == AppMode::Server {
                            ui.label("Audio Device:");
                            let prev_device = self.selected_device;
                            ui.add_enabled_ui(!self.shared_state.is_connected(), |ui| {
                                egui::ComboBox::from_id_salt("device_select")
                                    .width(340.0)
                                    .selected_text(
                                        self.devices.get(self.selected_device)
                                            .cloned()
                                            .unwrap_or_else(|| "Default".to_string())
                                    )
                                    .show_ui(ui, |ui| {
                                        for (i, device) in self.devices.iter().enumerate() {
                                            ui.selectable_value(&mut self.selected_device, i, device);
                                        }
                                    });
                            });
                            if prev_device != self.selected_device {
                                self.save_settings();
                            }
                        }
                        
                        // Client mode: Server IP
                        if self.mode == AppMode::Client {
                            ui.horizontal(|ui| {
                                ui.label("Server IP:");
                                let prev_ip = self.server_ip.clone();
                                ui.add_enabled_ui(!self.shared_state.is_connected(), |ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.server_ip)
                                        .desired_width(200.0)
                                        .hint_text("Auto-discover or enter IP"));
                                });
                                if prev_ip != self.server_ip {
                                    self.save_settings();
                                }
                            });
                        }
                        
                        ui.add_space(8.0);
                        
                        // Volume slider
                        ui.horizontal(|ui| {
                            ui.label("Volume:");
                            let prev_volume = self.volume;
                            ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0)
                                .show_value(false));
                            ui.label(format!("{}%", (self.volume * 100.0) as u32));
                            if (prev_volume - self.volume).abs() > 0.001 {
                                self.save_settings();
                            }
                        });
                });
                ui.add_space(10.0);
                
                // Status Section
                egui::Frame::none()
                    .fill(ui.style().visuals.extreme_bg_color)
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        // Audio level meter
                        ui.horizontal(|ui| {
                            ui.label("Level:");
                            let level = self.shared_state.get_audio_level();
                            let width = 300.0;
                            let height = 16.0;
                            let (response, painter) = ui.allocate_painter(
                                egui::vec2(width, height),
                                egui::Sense::hover()
                            );
                            
                            painter.rect_filled(response.rect, 4.0, egui::Color32::from_gray(40));
                            
                            let level_width = width * level.clamp(0.0, 1.0);
                            let color = if level < 0.7 {
                                egui::Color32::from_rgb(0, 200, 100)
                            } else if level < 0.9 {
                                egui::Color32::from_rgb(255, 200, 0)
                            } else {
                                egui::Color32::from_rgb(255, 50, 50)
                            };
                            
                            if level_width > 0.0 {
                                painter.rect_filled(
                                    egui::Rect::from_min_size(response.rect.min, egui::vec2(level_width, height)),
                                    4.0,
                                    color
                                );
                            }
                        });
                        
                        ui.add_space(8.0);
                        
                        // Status text
                        let (status_text, status_color) = self.get_status_display();
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("●").color(status_color).size(16.0));
                            ui.label(egui::RichText::new(&status_text).size(13.0));
                        });
                        
                        // Stats when connected
                        if self.shared_state.is_connected() {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(format!("📊 {}ms", self.shared_state.get_latency()));
                                ui.separator();
                                ui.label(format!("📦 {}/s", self.shared_state.get_packets_per_sec()));
                                ui.separator();
                                let bytes = self.shared_state.get_bytes_transferred();
                                let mb = bytes as f64 / 1_048_576.0;
                                ui.label(format!("📤 {:.1}MB", mb));
                            });
                        }
                });
                ui.add_space(15.0);
                
                // Control Buttons
                ui.horizontal(|ui| {
                    let is_running = self.shared_state.is_connected();
                    let button_text = if is_running { "⏹ STOP" } else { "▶ START" };
                    let button_color = if is_running {
                        egui::Color32::from_rgb(200, 50, 50)
                    } else {
                        egui::Color32::from_rgb(50, 150, 50)
                    };
                    
                    if ui.add_sized(
                        [100.0, 40.0],
                        egui::Button::new(egui::RichText::new(button_text).size(16.0).strong())
                            .fill(button_color)
                    ).clicked() {
                        if is_running {
                            self.stop_streaming();
                        } else {
                            self.start_streaming();
                        }
                    }
                    
                    ui.add_space(8.0);
                    
                    if ui.add_sized(
                        [90.0, 40.0],
                        egui::Button::new(egui::RichText::new("⚙ Settings").size(14.0))
                    ).clicked() {
                        self.show_settings = true;
                    }
                    
                    ui.add_space(8.0);
                    
                    if ui.add_sized(
                        [100.0, 40.0],
                        egui::Button::new(egui::RichText::new("🗕 Hide").size(14.0))
                    ).clicked() {
                        log::info!("Hide button clicked - minimizing to tray");
                        self.window_visible = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                });
                
                ui.add_space(15.0);
                
                // Help section (collapsed)
                ui.collapsing("ℹ Help", |ui| {
                    ui.label("• Server: Stream audio FROM this computer");
                    ui.label("• Client: Receive audio ON this computer");
                    ui.add_space(4.0);
                    ui.label("💡 Use tray icon to restore hidden window");
                });
            });
        });
        
        // Settings window
        if self.show_settings {
            egui::Window::new("⚙ Settings")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Server Name:");
                    ui.text_edit_singleline(&mut self.config.server_name);
                    ui.add_space(10.0);
                    
                    ui.label("Buffer Size (ms):");
                    ui.add(egui::Slider::new(&mut self.config.buffer_size_ms, 1..=50));
                    ui.add_space(10.0);
                    
                    ui.label("Bitrate (kbps):");
                    ui.add(egui::Slider::new(&mut self.config.bitrate_kbps, 64..=320));
                    ui.add_space(15.0);
                    
                    // Windows startup setting
                    let prev_startup = self.config.start_with_windows;
                    ui.checkbox(&mut self.config.start_with_windows, "Start with Windows");
                    if prev_startup != self.config.start_with_windows {
                        self.startup_changed = true;
                    }
                    
                    ui.checkbox(&mut self.config.start_minimized, "Start minimized to tray");
                    ui.add_space(10.0);
                    
                    // Auto-start options
                    ui.label("Auto-Start on Launch:");
                    ui.checkbox(&mut self.config.auto_start_server, "Auto-start Server (when in Server mode)");
                    ui.checkbox(&mut self.config.auto_start_client, "Auto-start Client and connect to last server (when in Client mode)");
                    ui.label(format!("Last saved server IP: {}", 
                        if self.config.last_server_ip.is_empty() { "(none)".to_string() } 
                        else { self.config.last_server_ip.clone() }));
                    ui.add_space(15.0);
                    
                    ui.add_space(15.0);
                    
                    // Console visibility toggle
                    ui.label("Console Window:");
                    ui.checkbox(&mut self.config.show_console, "Show console on startup");
                    ui.label("⚠️  Restart required when changed");
                    ui.label("When enabled, a console window will appear alongside the GUI.");
                    ui.label("When disabled, only the GUI will be visible (default).");
                    ui.add_space(15.0);
                    
                    ui.horizontal(|ui| {
                        if ui.button("💾 Save").clicked() {
                            self.apply_startup_setting();
                            self.save_settings();
                            self.show_settings = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_settings = false;
                            // Reload config to discard changes
                            self.config = Config::load().unwrap_or_default();
                            self.startup_changed = false;
                        }
                    });
                });
        }
    }
    
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Save window position and size
        if self.save_window_state {
            // Note: We'd need to get the actual position from egui, but for now we save other settings
            self.save_settings();
        }
        
        // Stop worker thread
        if self.worker_handle.is_some() {
            self.send_command(WorkerCommand::Stop);
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Background worker thread
fn worker_loop(
    cmd_rx: Receiver<WorkerCommand>,
    resp_tx: Sender<WorkerResponse>,
    shared_state: SharedState,
) {
    let mut current_task: Option<JoinHandle<()>> = None;
    let mut shutdown_tx: Option<Sender<()>> = None;
    
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WorkerCommand::StartServer { device } => {
                // Stop any existing task
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                if let Some(handle) = current_task.take() {
                    let _ = handle.join();
                }
                
                // Kill any existing instances using our ports
                if let Err(e) = kill_existing_instances() {
                    log::warn!("Failed to kill existing instances: {}", e);
                }
                
                // Ensure ports are free
                if let Err(e) = ensure_port_free(59100) {
                    let _ = resp_tx.send(WorkerResponse::Error(e.to_string()));
                    continue;
                }
                if let Err(e) = ensure_port_free(59200) {
                    let _ = resp_tx.send(WorkerResponse::Error(e.to_string()));
                    continue;
                }
                
                let (tx, rx) = channel::<()>();
                shutdown_tx = Some(tx);
                
                let state = shared_state.clone();
                let resp = resp_tx.clone();
                
                current_task = Some(thread::spawn(move || {
                    if let Err(e) = run_server_worker(device, state.clone(), rx, resp.clone()) {
                        log::error!("Server error: {}", e);
                        let _ = resp.send(WorkerResponse::Error(e.to_string()));
                        state.set_connected(false);
                    }
                }));
            }
            WorkerCommand::StartClient { server_ip } => {
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                if let Some(handle) = current_task.take() {
                    let _ = handle.join();
                }
                
                let (tx, rx) = channel::<()>();
                shutdown_tx = Some(tx);
                
                let state = shared_state.clone();
                let resp = resp_tx.clone();
                
                current_task = Some(thread::spawn(move || {
                    if let Err(e) = run_client_worker(server_ip, state.clone(), rx, resp.clone()) {
                        log::error!("Client error: {}", e);
                        let _ = resp.send(WorkerResponse::Error(e.to_string()));
                        state.set_connected(false);
                    }
                }));
            }
            WorkerCommand::Stop => {
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                if let Some(handle) = current_task.take() {
                    let _ = handle.join();
                }
                // Wait a bit for sockets to be fully released
                thread::sleep(Duration::from_millis(500));
                shared_state.set_connected(false);
                shared_state.set_status("Stopped");
                let _ = resp_tx.send(WorkerResponse::Stopped);
            }
        }
    }
}

/// Server worker thread
fn run_server_worker(
    device_name: Option<String>,
    state: SharedState,
    shutdown_rx: Receiver<()>,
    resp_tx: Sender<WorkerResponse>,
) -> anyhow::Result<()> {
    use std::net::UdpSocket;
    use std::collections::HashSet;
    
    state.set_status("Starting Server...");
    
    // Start discovery broadcast
    DiscoveryService::start_server_broadcast(
        hostname::get()?.to_string_lossy().to_string()
    )?;
    
    // Create audio capture
    let capture = AudioCapture::new(device_name)?;
    let mut encoder = OpusEncoder::new()?;
    
    let (frame_tx, frame_rx) = channel::<AudioFrame>();
    let (packet_tx, packet_rx) = channel::<(Vec<u8>, u64)>();
    
    let _capture_stream = capture.start(frame_tx)?;
    
    // Encoder thread
    thread::spawn(move || {
        while let Ok(frame) = frame_rx.recv() {
            match encoder.encode(&frame) {
                Ok(encoded) => {
                    if packet_tx.send((encoded, frame.timestamp)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Encoding error: {}", e);
                }
            }
        }
    });
    
    // Create socket with SO_REUSEADDR to allow quick restarts
    let socket = match create_listener_socket(59200) {
        Ok(s) => {
            log::info!("Streaming server bound to port 59200 (with SO_REUSEADDR)");
            Arc::new(s)
        }
        Err(e) => {
            anyhow::bail!("Failed to bind streaming socket: {}. Try waiting a few seconds before restarting.", e);
        }
    };
    socket.set_nonblocking(true)?;
    
    let clients: Arc<Mutex<HashSet<std::net::SocketAddr>>> = Arc::new(Mutex::new(HashSet::new()));
    let clients_clone = clients.clone();
    let socket_clone = socket.clone();
    
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match socket_clone.recv_from(&mut buf) {
                Ok((len, addr)) => {
                    if len == 1 && buf[0] == 0x01 {
                        let mut clients = clients_clone.lock().unwrap();
                        if clients.insert(addr) {
                            log::info!("New client connected: {}", addr);
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {}
            }
        }
    });
    
    state.set_connected(true);
    state.set_status("Streaming");
    let _ = resp_tx.send(WorkerResponse::Started);
    
    let mut sequence = 0u32;
    let mut packets_sent = 0u32;
    let mut last_stats = Instant::now();
    
    while let Ok((payload, timestamp)) = packet_rx.recv() {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        
        let packet = AudioPacket::new(sequence, timestamp, payload);
        let bytes = packet.to_bytes();
        
        let clients_list = clients.lock().unwrap().clone();
        
        for client_addr in clients_list.iter() {
            let _ = socket.send_to(&bytes, client_addr);
        }
        
        packets_sent += 1;
        state.add_bytes(bytes.len() as u64);
        sequence = sequence.wrapping_add(1);
        
        if last_stats.elapsed().as_secs() >= 1 {
            let pps = packets_sent;
            packets_sent = 0;
            last_stats = Instant::now();
            let _ = resp_tx.send(WorkerResponse::Stats { packets: pps, latency: 0 });
        }
    }
    
    Ok(())
}

/// Client worker thread
fn run_client_worker(
    server_ip: Option<String>,
    state: SharedState,
    shutdown_rx: Receiver<()>,
    resp_tx: Sender<WorkerResponse>,
) -> anyhow::Result<()> {
    state.set_status("Discovering...");
    
    let server_addr = if let Some(ip) = server_ip {
        let addr: std::net::SocketAddr = format!("{}:59200", ip).parse()?;
        state.set_status(&format!("Connecting to {}...", addr));
        addr
    } else {
        let discovery = DiscoveryService::new();
        discovery.start_client_listen()?;
        
        let start = Instant::now();
        let server = loop {
            if shutdown_rx.try_recv().is_ok() {
                anyhow::bail!("Shutdown requested");
            }
            
            if start.elapsed().as_secs() > 10 {
                anyhow::bail!("No servers found. Try entering an IP address directly.");
            }
            
            thread::sleep(Duration::from_millis(100));
            
            let servers = discovery.get_servers();
            if !servers.is_empty() {
                break servers.into_iter().next().unwrap();
            }
        };
        
        let mut addr = server.address;
        addr.set_port(59200);
        state.set_status(&format!("Found {}, connecting...", server.name));
        addr
    };
    
    // Save the server IP for auto-reconnect on next launch
    let server_ip_string = server_addr.ip().to_string();
    let _ = resp_tx.send(WorkerResponse::ConnectedToServer { ip: server_ip_string });
    
    let playback = AudioPlayback::new()?;
    let mut decoder = OpusDecoder::new()?;
    
    let (packet_tx, packet_rx) = channel::<AudioPacket>();
    let (frame_tx, frame_rx) = channel::<AudioFrame>();
    
    let _playback_stream = playback.start(frame_rx)?;
    
    let state_decoder = state.clone();
    thread::spawn(move || {
        while let Ok(packet) = packet_rx.recv() {
            match decoder.decode(&packet.payload, packet.timestamp) {
                Ok(frame) => {
                    let sum_sq: f32 = frame.samples.iter().map(|&s| s * s).sum();
                    let rms = (sum_sq / frame.samples.len().max(1) as f32).sqrt();
                    state_decoder.set_audio_level(rms.min(1.0));
                    
                    if frame_tx.send(frame).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Decoding error: {}", e);
                }
            }
        }
    });
    
    let client = StreamingClient::new(packet_tx)?;
    let connection_state = client.get_connection_state();
    
    // Don't set connected yet - wait for actual data
    state.set_status("Connecting...");
    let _ = resp_tx.send(WorkerResponse::Started);
    
    let mut last_stats = Instant::now();
    let mut packets_received = 0u32;
    let mut was_connected = false;
    let mut reconnect_count = 0;
    
    let (done_tx, done_rx) = channel::<anyhow::Result<()>>();
    
    // Spawn the client connection in a separate thread
    thread::spawn(move || {
        let result = client.connect_and_receive(server_addr);
        let _ = done_tx.send(result);
    });
    
    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        
        // Check connection state and update UI accordingly
        let is_connected = connection_state.load(Ordering::SeqCst);
        
        if is_connected && !was_connected {
            // Just connected
            state.set_connected(true);
            state.set_status("Connected");
            reconnect_count = 0;
            was_connected = true;
            log::info!("✅ Client fully connected - audio stream established");
        } else if !is_connected && was_connected {
            // Just disconnected
            state.set_connected(false);
            state.set_status("Reconnecting...");
            reconnect_count += 1;
            was_connected = false;
            log::warn!("⚠️  Connection lost - attempting reconnect #{} in 3 seconds...", reconnect_count);
        }
        
        if let Ok(result) = done_rx.try_recv() {
            // The connection loop ended (shouldn't happen with auto-reconnect, but handle it)
            match result {
                Ok(()) => {
                    log::info!("Client connection closed normally");
                }
                Err(e) => {
                    log::error!("Client connection error: {} - will attempt reconnect", e);
                    state.set_status(&format!("Connection error: {} - Reconnecting...", e));
                }
            }
            // With auto-reconnect, the inner loop should never really end,
            // but if it does, we'll exit and let the caller restart
            break;
        }
        
        if last_stats.elapsed().as_secs() >= 1 {
            last_stats = Instant::now();
            let _ = resp_tx.send(WorkerResponse::Stats { 
                packets: packets_received, 
                latency: state.get_latency() 
            });
            packets_received = 0;
        }
        
        thread::sleep(Duration::from_millis(100));
    }
    
    state.set_connected(false);
    Ok(())
}

/// Run the GUI application
pub fn run_gui(config: Config, start_minimized: bool) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 520.0])
            .with_resizable(false)  // Disable window resizing
            .with_icon(load_icon()),
        ..Default::default()
    };
    
    eframe::run_native(
        "AudioSender",
        options,
        Box::new(|cc| Ok(Box::new(AudioSenderApp::new(cc, config, start_minimized)))),
    )
}

fn load_icon() -> egui::IconData {
    // Load the PNG logo embedded at compile time
    let png_data = include_bytes!("../logo.png");
    
    // Decode PNG using the image crate
    let image = image::load_from_memory(png_data)
        .expect("Failed to load logo.png")
        .into_rgba8();
    
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    
    egui::IconData {
        rgba,
        width,
        height,
    }
}

