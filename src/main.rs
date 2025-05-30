// SPDX-License-Identifier: Apache-2.0
//
// kvm-rs: Minimal KVM‑IP server for OpenBMC
//
// Build: cargo build --release --target armv7-unknown-linux-gnueabihf
// Run  : systemd unit (ver §4)

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    routing::get, Router,
};
use clap::Parser;
use std::sync::Arc;
use tokio::{fs::File, io::AsyncReadExt, sync::broadcast};
use zbus::Connection;

/// KVM-RS: Minimal KVM-IP server for OpenBMC
#[derive(Parser, Debug)]
#[command(name = "kvm-rs")]
#[command(about = "Minimal KVM-IP server for OpenBMC")]
struct Args {
    /// Video device path (framebuffer)
    #[arg(short = 'v', long = "video", default_value = "/dev/fb0")]
    video_device: String,

    /// HID gadget device for keyboard input
    #[arg(short = 'k', long = "keyboard-hid", default_value = "/dev/hidg0")]
    keyboard_hid: String,

    /// HID gadget device for mouse input  
    #[arg(short = 'm', long = "mouse-hid", default_value = "/dev/hidg1")]
    mouse_hid: String,

    /// Port to listen on
    #[arg(short = 'p', long = "port", default_value = "8443")]
    port: u16,

    /// Bind address
    #[arg(short = 'b', long = "bind", default_value = "0.0.0.0")]
    bind_address: String,
}

/// HID device manager for keyboard and mouse input
#[derive(Clone)]
struct HidManager {
    keyboard_device: String,
    mouse_device: String,
}

impl HidManager {
    fn new(keyboard_device: String, mouse_device: String) -> Self {
        Self {
            keyboard_device,
            mouse_device,
        }
    }

    /// Send keyboard input to HID gadget device
    async fn send_keyboard_input(&self, data: &[u8]) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;
        
        // TODO: In production, validate HID report format
        if data.len() < 8 {
            return Err(anyhow::anyhow!("Keyboard HID report must be at least 8 bytes"));
        }
        
        match tokio::fs::OpenOptions::new()
            .write(true)
            .open(&self.keyboard_device)
            .await
        {
            Ok(mut file) => {
                file.write_all(data).await?;
                file.flush().await?;
                println!("Sent keyboard input to {}: {} bytes", self.keyboard_device, data.len());
            }
            Err(e) => {
                eprintln!("Failed to open keyboard device {}: {}", self.keyboard_device, e);
                return Err(e.into());
            }
        }
        Ok(())
    }

    /// Send mouse input to HID gadget device
    async fn send_mouse_input(&self, data: &[u8]) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;
        
        // TODO: In production, validate HID report format
        if data.len() < 4 {
            return Err(anyhow::anyhow!("Mouse HID report must be at least 4 bytes"));
        }
        
        match tokio::fs::OpenOptions::new()
            .write(true)
            .open(&self.mouse_device)
            .await
        {
            Ok(mut file) => {
                file.write_all(data).await?;
                file.flush().await?;
                println!("Sent mouse input to {}: {} bytes", self.mouse_device, data.len());
            }
            Err(e) => {
                eprintln!("Failed to open mouse device {}: {}", self.mouse_device, e);
                return Err(e.into());
            }
        }
        Ok(())
    }
}

/// Shared framebuffer broadcaster
struct DisplayHub {
    tx: broadcast::Sender<Vec<u8>>,
}

impl DisplayHub {
    async fn spawn(self: Arc<Self>, fb_path: String) -> anyhow::Result<()> {
        let mut file = File::open(fb_path).await?;
        let mut buf   = vec![0u8; 1920 * 1080 * 4]; // 1080p RGBA

        loop {
            file.read_exact(&mut buf).await?;          // read raw frame
            let _ = self.tx.send(buf.clone());         // broadcast
            tokio::time::sleep(std::time::Duration::from_millis(33)).await; // ~30 fps
        }
    }
}

/// WebSocket handler
async fn kvm_ws(
    ws: WebSocketUpgrade,
    hub: Arc<DisplayHub>,
    hid_manager: HidManager,
) -> axum::response::Response {
    ws.on_upgrade(|mut socket: WebSocket| async move {
        let mut rx = hub.tx.subscribe();
        // TODO: Handshake RFB / VNC here
        
        loop {
            tokio::select! {
                // Send framebuffer data to client
                frame = rx.recv() => {
                    match frame {
                        Ok(frame_data) => {
                            if socket.send(Message::Binary(frame_data.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                
                // Receive input from client
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            // TODO: Parse input data and determine if it's keyboard or mouse
                            // For now, just show how the HID devices would be used
                            if data.len() > 0 {
                                match data[0] {
                                    0x01 => { // Example: keyboard input
                                        if let Err(e) = hid_manager.send_keyboard_input(&data[1..]).await {
                                            eprintln!("Keyboard input error: {}", e);
                                        }
                                    }
                                    0x02 => { // Example: mouse input
                                        if let Err(e) = hid_manager.send_mouse_input(&data[1..]).await {
                                            eprintln!("Mouse input error: {}", e);
                                        }
                                    }
                                    _ => {
                                        println!("Unknown input type: {}", data[0]);
                                    }
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(e)) => {
                            eprintln!("WebSocket error: {}", e);
                            break;
                        }
                        _ => {} // Ignore other message types
                    }
                }
            }
        }
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    println!("KVM‑RS starting with:");
    println!("  Video device: {}", args.video_device);
    println!("  Keyboard HID: {}", args.keyboard_hid);
    println!("  Mouse HID: {}", args.mouse_hid);
    println!("  Listening on: {}:{}", args.bind_address, args.port);

    // Validate that devices exist (optional check)
    if !std::path::Path::new(&args.video_device).exists() {
        eprintln!("Warning: Video device {} does not exist", args.video_device);
    }
    if !std::path::Path::new(&args.keyboard_hid).exists() {
        eprintln!("Warning: Keyboard HID device {} does not exist", args.keyboard_hid);
    }
    if !std::path::Path::new(&args.mouse_hid).exists() {
        eprintln!("Warning: Mouse HID device {} does not exist", args.mouse_hid);
    }

    // 1. Conecta a DBus para verificar sesión válida (Redfish)
    let _dbus: Connection = Connection::system().await?;

    // 2. Framebuffer broadcaster
    let hub = Arc::new(DisplayHub {
        tx: broadcast::channel(16).0,
    });
    let video_device = args.video_device.clone();
    tokio::spawn(hub.clone().spawn(video_device));

    // 3. HID manager
    let hid_manager = HidManager::new(args.keyboard_hid.clone(), args.mouse_hid.clone());

    // 4. Servidor HTTP → WS
    let app = Router::new()
        .route("/kvm/0", get({
            let h = hub.clone();
            let hid_mgr = hid_manager.clone();
            move |ws| kvm_ws(ws, h, hid_mgr)
        }));

    println!("KVM‑RS listening on {}:{}", args.bind_address, args.port);
    
    // Create TCP listener with configurable address and port
    let bind_addr = format!("{}:{}", args.bind_address, args.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await
        .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", bind_addr, e))?;
    
    // Start the server using axum::serve
    axum::serve(listener, app).await?;

    Ok(())
}
