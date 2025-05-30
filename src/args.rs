// SPDX-License-Identifier: Apache-2.0
//
// Command line argument parsing for kvm-rs

use clap::Parser;

/// KVM-RS: Minimal KVM-IP server for OpenBMC
#[derive(Parser, Debug)]
#[command(name = "kvm-rs")]
#[command(about = "Minimal KVM-IP server for OpenBMC")]
pub struct Args {
    /// Video device path (V4L2 video device or framebuffer)
    #[arg(short = 'v', long = "video", default_value = "/dev/video0")]
    pub video_device: String,

    /// Force framebuffer mode instead of auto-detection
    #[arg(long = "force-framebuffer")]
    pub force_framebuffer: bool,

    /// HID gadget device for keyboard input
    #[arg(short = 'k', long = "keyboard-hid", default_value = "/dev/hidg0")]
    pub keyboard_hid: String,

    /// HID gadget device for mouse input  
    #[arg(short = 'm', long = "mouse-hid", default_value = "/dev/hidg1")]
    pub mouse_hid: String,

    /// Port to listen on
    #[arg(short = 'p', long = "port", default_value = "8443")]
    pub port: u16,

    /// VNC server port
    #[arg(long = "vnc-port", default_value = "5900")]
    pub vnc_port: u16,

    /// Bind address
    #[arg(short = 'b', long = "bind", default_value = "0.0.0.0")]
    pub bind_address: String,
}

impl Args {
    /// Validate that the specified device paths exist
    pub fn validate_devices(&self) {
        if !std::path::Path::new(&self.video_device).exists() {
            eprintln!("Warning: Video device {} does not exist", self.video_device);
        }
        if !std::path::Path::new(&self.keyboard_hid).exists() {
            eprintln!("Warning: Keyboard HID device {} does not exist", self.keyboard_hid);
        }
        if !std::path::Path::new(&self.mouse_hid).exists() {
            eprintln!("Warning: Mouse HID device {} does not exist", self.mouse_hid);
        }
    }

    /// Print configuration summary
    pub fn print_config(&self) {
        println!("KVM‑RS starting with:");
        println!("  Video device: {}", self.video_device);
        if self.force_framebuffer {
            println!("  Video mode: Framebuffer (forced)");
        } else {
            println!("  Video mode: Auto-detect (V4L2 preferred, framebuffer fallback)");
        }
        println!("  Keyboard HID: {}", self.keyboard_hid);
        println!("  Mouse HID: {}", self.mouse_hid);
        println!("  WebSocket listening on: {}:{}", self.bind_address, self.port);
        println!("  VNC listening on: {}:{}", self.bind_address, self.vnc_port);
    }
}
