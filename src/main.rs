mod audio;
mod network;
mod config;
mod server;
mod client;
mod gui;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::info;

#[derive(Parser)]
#[command(name = "AudioSender")]
#[command(about = "Ultra-low latency audio streaming for Windows", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    
    /// Launch GUI mode (default if no command specified)
    #[arg(long)]
    gui: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run as server (audio source)
    Server {
        /// Optional audio device name to capture from
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Run as client (audio receiver)
    Client {
        /// Optional server IP to connect to directly
        #[arg(short, long)]
        connect: Option<String>,
    },
    /// List available audio devices
    ListDevices,
    /// Launch GUI mode
    Gui,
}

fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();
    
    info!("AudioSender v{}", env!("CARGO_PKG_VERSION"));
    
    // Load configuration
    let config = config::Config::load()?;
    
    // Parse command line arguments
    let cli = Cli::parse();
    
    // If no command specified or --gui flag, launch GUI
    if cli.gui || cli.command.is_none() {
        info!("Launching GUI mode");
        if let Err(e) = gui::app::run_gui() {
            eprintln!("GUI error: {}", e);
        }
        return Ok(());
    }
    
    match cli.command.unwrap() {
        Commands::Server { device } => {
            info!("Starting in SERVER mode");
            if let Some(ref d) = device {
                info!("Selected audio device: '{}'", d);
            }
            server::run_server(config, device)?;
        }
        Commands::Client { connect } => {
            info!("Starting in CLIENT mode");
            client::run_client(config, connect)?;
        }
        Commands::ListDevices => {
            let devices = crate::audio::capture::AudioCapture::list_devices()?;
            println!("\nAvailable Output Audio Devices:");
            println!("---------------------------------");
            if devices.is_empty() {
                println!("No devices found.");
            } else {
                for (i, dev) in devices.iter().enumerate() {
                    println!("  {}. {}", i + 1, dev);
                }
            }
            println!("\nUsage: audio_sender.exe server --device \"Device Name\"");
        }
        Commands::Gui => {
            info!("Launching GUI mode");
            if let Err(e) = gui::app::run_gui() {
                eprintln!("GUI error: {}", e);
            }
        }
    }
    
    Ok(())
}
