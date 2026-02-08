// Build script to compile Windows resource file with logo icon
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    // Only run on Windows
    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    
    // Use the existing logo.ico file
    let logo_path = Path::new(&manifest_dir).join("src").join("logo.ico");
    let logo_path_str = logo_path.display().to_string().replace("\\", "/");
    
    // Create .rc file with:
    // - ID 1 for the application exe icon (shown in Explorer)
    // - Named "app-icon" for the tray icon
    let rc_content = format!(
        "1 ICON \"{}\"\napp-icon ICON \"{}\"",
        logo_path_str, logo_path_str
    );
    let rc_path = Path::new(&manifest_dir).join("app.rc");
    fs::write(&rc_path, &rc_content).expect("Failed to write RC file");
    
    // Compile the resource file
    let _ = embed_resource::compile(&rc_path, embed_resource::NONE);
    
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/logo.ico");
}

