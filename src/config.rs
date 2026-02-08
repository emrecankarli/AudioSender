use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::gui::app::AppMode;

/// Application configuration
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    // Server settings
    pub server_name: String,
    pub buffer_size_ms: u32,
    pub bitrate_kbps: u32,
    
    // Startup settings
    pub start_with_windows: bool,
    pub start_minimized: bool,
    
    // Last used settings (persisted session)
    pub last_mode: String, // "server" or "client"
    pub last_device_index: usize,
    pub last_server_ip: String,
    pub last_volume: f32,
    pub last_window_pos: Option<(f32, f32)>,
    pub last_window_size: Option<(f32, f32)>,
    
    // Auto-connect settings
    pub auto_connect: bool,
    pub auto_start_server: bool,
    pub auto_start_client: bool,
    
    // Console visibility
    pub show_console: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_name: hostname::get()
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| "AudioSender".to_string()),
            buffer_size_ms: 5,
            bitrate_kbps: 128,
            start_with_windows: false,
            start_minimized: false,
            last_mode: "server".to_string(),
            last_device_index: 0,
            last_server_ip: "".to_string(),
            last_volume: 0.8,
            last_window_pos: None,
            last_window_size: None,
            auto_connect: false,
            auto_start_server: false,
            auto_start_client: false,
            show_console: false,  // Default: hide console, show only GUI
        }
    }
}

impl Config {
    /// Get the config file path
    fn config_path() -> Result<PathBuf> {
        let proj_dirs = ProjectDirs::from("com", "AudioSender", "AudioSender")
            .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?;
        
        let config_dir = proj_dirs.config_dir();
        fs::create_dir_all(config_dir)?;
        
        Ok(config_dir.join("config.toml"))
    }
    
    /// Load configuration from file, or create default if not exists
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        
        if path.exists() {
            let contents = fs::read_to_string(&path)?;
            // Try to parse, if it fails (e.g., missing fields), use defaults for missing fields
            let config: Config = match toml::from_str(&contents) {
                Ok(c) => c,
                Err(_) => {
                    log::warn!("Config file has missing fields, merging with defaults");
                    // Try partial load by deserializing to a Value first
                    let mut default_config = Self::default();
                    // Try to preserve existing values from the old config
                    if let Ok(value) = contents.parse::<toml::Value>() {
                        if let Some(table) = value.as_table() {
                            // Copy over any values that exist in the old config
                            if let Some(v) = table.get("server_name").and_then(|v| v.as_str()) {
                                default_config.server_name = v.to_string();
                            }
                            if let Some(v) = table.get("auto_connect").and_then(|v| v.as_bool()) {
                                default_config.auto_connect = v;
                            }
                            if let Some(v) = table.get("start_with_windows").and_then(|v| v.as_bool()) {
                                default_config.start_with_windows = v;
                            }
                            if let Some(v) = table.get("buffer_size_ms").and_then(|v| v.as_integer()) {
                                default_config.buffer_size_ms = v as u32;
                            }
                            if let Some(v) = table.get("bitrate_kbps").and_then(|v| v.as_integer()) {
                                default_config.bitrate_kbps = v as u32;
                            }
                        }
                    }
                    default_config.save()?;
                    default_config
                }
            };
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }
    
    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }
    
    /// Get last mode as AppMode
    pub fn get_last_mode(&self) -> AppMode {
        match self.last_mode.as_str() {
            "client" => AppMode::Client,
            _ => AppMode::Server,
        }
    }
    
    /// Set last mode from AppMode
    pub fn set_last_mode(&mut self, mode: AppMode) {
        self.last_mode = match mode {
            AppMode::Server => "server".to_string(),
            AppMode::Client => "client".to_string(),
        };
    }
}

/// Windows startup registry management
#[cfg(windows)]
pub mod startup {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE, KEY_READ};
    use winreg::RegKey;
    use std::env;
    
    const REG_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const APP_NAME: &str = "AudioSender";
    
    /// Check if the app is set to start with Windows
    pub fn is_startup_enabled() -> Result<bool> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu.open_subkey_with_flags(REG_PATH, KEY_READ)?;
        
        match run_key.get_value::<String, _>(APP_NAME) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
    
    /// Enable startup with Windows
    pub fn enable_startup() -> Result<()> {
        let exe_path = env::current_exe()?;
        let exe_path_str = exe_path.to_string_lossy();
        
        // Add --minimized flag for startup
        let value = format!("\"{}\" --minimized", exe_path_str);
        
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu.create_subkey_with_flags(REG_PATH, KEY_WRITE)?;
        run_key.set_value(APP_NAME, &value)?;
        
        log::info!("Startup enabled: {}", value);
        Ok(())
    }
    
    /// Disable startup with Windows
    pub fn disable_startup() -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu.create_subkey_with_flags(REG_PATH, KEY_WRITE)?;
        run_key.delete_value(APP_NAME)?;
        
        log::info!("Startup disabled");
        Ok(())
    }
    
    /// Toggle startup setting
    pub fn toggle_startup(enabled: bool) -> Result<()> {
        if enabled {
            enable_startup()
        } else {
            disable_startup()
        }
    }
}

#[cfg(not(windows))]
pub mod startup {
    use super::*;
    
    pub fn is_startup_enabled() -> Result<bool> {
        Ok(false)
    }
    
    pub fn enable_startup() -> Result<()> {
        anyhow::bail!("Windows startup is only supported on Windows")
    }
    
    pub fn disable_startup() -> Result<()> {
        anyhow::bail!("Windows startup is only supported on Windows")
    }
    
    pub fn toggle_startup(_enabled: bool) -> Result<()> {
        Ok(())
    }
}
