use anyhow::Result;
use log::{info, warn, error};
use std::net::TcpListener;
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::network::socket::create_listener_socket;

/// Check if a port is already in use
pub fn is_port_in_use(port: u16) -> bool {
    TcpListener::bind(format!("0.0.0.0:{}", port)).is_err()
}

/// Find and kill process using a specific port (Windows only)
#[cfg(windows)]
pub fn kill_process_using_port(port: u16) -> Result<bool> {
    
    // Step 1: Find the PID using netstat
    let netstat_output = Command::new("cmd")
        .args(&["/C", &format!("netstat -ano | findstr :{}", port)])
        .output()?;
    
    // netstat may return non-zero if no matches, that's ok
    let output_str = String::from_utf8_lossy(&netstat_output.stdout);
    
    if output_str.trim().is_empty() {
        info!("No process found using port {}", port);
        return Ok(false);
    }
    
    // Parse the output to find the PID
    // Format: Proto  Local Address          Foreign Address        State           PID
    // Example: TCP    0.0.0.0:59200          0.0.0.0:0              LISTENING       12345
    
    for line in output_str.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            // Check if this line contains our port (and is LISTENING)
            if line.contains(&format!(":{}", port)) && line.contains("LISTENING") {
                if let Some(pid_str) = parts.last() {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        // Don't kill system processes (PID < 1000)
                        if pid < 1000 {
                            warn!("Skipping system process with PID {}", pid);
                            continue;
                        }
                        
                        // Don't kill ourselves!
                        let current_pid = std::process::id();
                        if pid == current_pid {
                            info!("Skipping self (PID {})", pid);
                            continue;
                        }
                        
                        info!("Found process {} using port {}", pid, port);
                        
                        // Step 2: Get process name
                        let tasklist_output = Command::new("cmd")
                            .args(&["/C", &format!("tasklist /FI \"PID eq {}\" /FO CSV /NH 2>nul", pid)])
                            .output()?;
                        
                        let tasklist_str = String::from_utf8_lossy(&tasklist_output.stdout);
                        if !tasklist_str.trim().is_empty() {
                            info!("Process info: {}", tasklist_str.trim());
                        }
                        
                        // Step 3: Kill the process
                        info!("Killing process {}...", pid);
                        let kill_result = Command::new("taskkill")
                            .args(&["/PID", &pid.to_string(), "/F"])  // /F = force
                            .creation_flags(0x08000000)  // CREATE_NO_WINDOW
                            .output()?;
                        
                        if kill_result.status.success() {
                            info!("Successfully killed process {} using port {}", pid, port);
                            
                            // Wait a moment for the port to be released
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            
                            // Verify the port is now free
                            if !is_port_in_use(port) {
                                return Ok(true);
                            } else {
                                warn!("Port {} still in use after killing process", port);
                                return Ok(false);
                            }
                        } else {
                            let error_str = String::from_utf8_lossy(&kill_result.stderr);
                            error!("Failed to kill process: {}", error_str);
                            // Continue trying other processes
                        }
                    }
                }
            }
        }
    }
    
    info!("No suitable process found to kill on port {}", port);
    Ok(false)
}

#[cfg(not(windows))]
pub fn kill_process_using_port(_port: u16) -> Result<bool> {
    Ok(false)
}

/// Ensure a port is free, killing any process using it if necessary
pub fn ensure_port_free(port: u16) -> Result<()> {
    if !is_port_in_use(port) {
        return Ok(());
    }
    
    warn!("Port {} is already in use. Attempting to free it...", port);
    
    // First try SO_REUSEADDR approach - this often works for TIME_WAIT sockets
    match create_listener_socket(port) {
        Ok(socket) => {
            info!("Port {} was in TIME_WAIT but SO_REUSEADDR allowed bind", port);
            drop(socket);
            return Ok(());
        }
        Err(_) => {
            // Port is truly in use by another process, try to kill it
            match kill_process_using_port(port) {
                Ok(true) => {
                    info!("Port {} is now free", port);
                    Ok(())
                }
                Ok(false) => {
                    // Check if port is now free (maybe it was released already)
                    if !is_port_in_use(port) {
                        info!("Port {} is now free (was released)", port);
                        return Ok(());
                    }
                    anyhow::bail!(
                        "Port {} is in use and could not be freed. \
                         Please close any other AudioSender instances or applications using this port.", 
                        port
                    )
                }
                Err(e) => {
                    anyhow::bail!(
                        "Failed to free port {}: {}. \
                         Please close any other AudioSender instances manually.", 
                        port, e
                    )
                }
            }
        }
    }
}

/// Check if another instance of AudioSender is already running
pub fn is_another_instance_running() -> bool {
    let current_pid = std::process::id();
    
    // Check both discovery and streaming ports
    for port in [59100, 59200] {
        if is_port_in_use(port) {
            // Check if it's us or another instance
            if let Ok(output) = Command::new("cmd")
                .args(&["/C", &format!("netstat -ano | findstr \"LISTENING\" | findstr :{}", port)])
                .output() 
            {
                let output_str = String::from_utf8_lossy(&output.stdout);
                for line in output_str.lines() {
                    if let Some(pid_str) = line.split_whitespace().last() {
                        if let Ok(pid) = pid_str.parse::<u32>() {
                            if pid != current_pid {
                                return true; // Another instance is running
                            }
                        }
                    }
                }
            }
        }
    }
    
    false
}

/// Kill any existing AudioSender processes (excluding current process)
#[cfg(windows)]
pub fn kill_existing_instances() -> Result<()> {
    let current_pid = std::process::id();
    info!("Checking for existing AudioSender instances (current PID: {})...", current_pid);
    
    // Kill processes using our ports (but not ourselves)
    let mut killed_any = false;
    
    if is_port_in_use(59200) {
        if kill_process_using_port(59200)? {
            killed_any = true;
        }
    }
    
    if is_port_in_use(59100) {
        if kill_process_using_port(59100)? {
            killed_any = true;
        }
    }
    
    // Also try to find and kill other audio_sender.exe processes by checking each PID
    // This is safer than taskkill /IM which would kill ourselves too
    let tasklist_output = Command::new("cmd")
        .args(&["/C", "tasklist /FI \"IMAGENAME eq audio_sender.exe\" /FO CSV /NH 2>nul"])
        .output()?;
    
    let tasklist_str = String::from_utf8_lossy(&tasklist_output.stdout);
    for line in tasklist_str.lines() {
        // Parse CSV: "audio_sender.exe","12345","Console","1","12,345 K"
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 2 {
            // Extract PID from quoted string
            let pid_str = parts[1].trim_matches('"');
            if let Ok(pid) = pid_str.parse::<u32>() {
                if pid != current_pid {
                    info!("Found other AudioSender instance with PID {}, killing...", pid);
                    let _ = Command::new("taskkill")
                        .args(&["/PID", &pid.to_string(), "/F"])
                        .creation_flags(0x08000000)
                        .output();
                    killed_any = true;
                }
            }
        }
    }
    
    if killed_any {
        info!("Existing instances cleaned up");
        std::thread::sleep(std::time::Duration::from_millis(500));
    } else {
        info!("No other instances found");
    }
    
    Ok(())
}

#[cfg(not(windows))]
pub fn kill_existing_instances() -> Result<()> {
    Ok(())
}
