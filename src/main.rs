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

    /// VNC server port
    #[arg(long = "vnc-port", default_value = "5900")]
    vnc_port: u16,

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

/// VNC Server handler for noVNC clients
struct VncHandler {
    hub: Arc<DisplayHub>,
    hid_manager: HidManager,
}

impl VncHandler {
    fn new(hub: Arc<DisplayHub>, hid_manager: HidManager) -> Self {
        Self {
            hub,
            hid_manager,
        }
    }

    async fn start_vnc_server(self, bind_addr: String, port: u16) -> anyhow::Result<()> {
        use tokio::net::TcpListener;
        
        let listener = TcpListener::bind(format!("{}:{}", bind_addr, port)).await?;
        println!("VNC server listening on {}:{}", bind_addr, port);

        while let Ok((mut stream, addr)) = listener.accept().await {
            println!("VNC client connected from: {}", addr);
            
            let hub = self.hub.clone();
            let hid_manager = self.hid_manager.clone();
            
            tokio::spawn(async move {
                if let Err(e) = Self::handle_vnc_client(&mut stream, hub, hid_manager).await {
                    eprintln!("VNC client error: {}", e);
                }
            });
        }
        
        Ok(())
    }

    async fn handle_vnc_client(
        stream: &mut tokio::net::TcpStream,
        hub: Arc<DisplayHub>,
        hid_manager: HidManager,
    ) -> anyhow::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // VNC handshake
        // Send RFB protocol version
        stream.write_all(b"RFB 003.008\n").await?;
        
        // Read client protocol version
        let mut version_buf = [0u8; 12];
        stream.read_exact(&mut version_buf).await?;
        println!("Client VNC version: {}", String::from_utf8_lossy(&version_buf));

        // Security handshake - no authentication for simplicity
        stream.write_all(&[1u8, 1u8]).await?; // 1 security type: None
        let mut security_choice = [0u8; 1];
        stream.read_exact(&mut security_choice).await?;
        
        if security_choice[0] != 1 {
            return Err(anyhow::anyhow!("Client chose unsupported security type"));
        }

        // Security result - OK
        stream.write_all(&[0u8, 0u8, 0u8, 0u8]).await?;

        // Read ClientInit
        let mut client_init = [0u8; 1];
        stream.read_exact(&mut client_init).await?;

        // Send ServerInit
        let server_init = Self::create_server_init();
        stream.write_all(&server_init).await?;

        // Start framebuffer updates and input handling
        Self::handle_vnc_session(stream, hub, hid_manager).await?;

        Ok(())
    }

    fn create_server_init() -> Vec<u8> {
        let mut init = Vec::new();
        
        // Framebuffer width (1920) - big endian
        init.extend_from_slice(&1920u16.to_be_bytes());
        // Framebuffer height (1080) - big endian  
        init.extend_from_slice(&1080u16.to_be_bytes());
        
        // Pixel format (32-bit RGBA)
        init.push(32); // bits per pixel
        init.push(24); // depth
        init.push(0);  // big endian flag (0 = little endian)
        init.push(1);  // true color flag
        init.extend_from_slice(&255u16.to_be_bytes()); // red max
        init.extend_from_slice(&255u16.to_be_bytes()); // green max
        init.extend_from_slice(&255u16.to_be_bytes()); // blue max
        init.push(16); // red shift
        init.push(8);  // green shift
        init.push(0);  // blue shift
        init.extend_from_slice(&[0u8; 3]); // padding

        // Desktop name
        let name = b"KVM-RS";
        init.extend_from_slice(&(name.len() as u32).to_be_bytes());
        init.extend_from_slice(name);
        
        init
    }

    async fn handle_vnc_session(
        stream: &mut tokio::net::TcpStream,
        hub: Arc<DisplayHub>,
        hid_manager: HidManager,
    ) -> anyhow::Result<()> {
        use tokio::io::AsyncReadExt;
        
        let mut rx = hub.tx.subscribe();
        let mut buffer = [0u8; 1024];
        
        loop {
            tokio::select! {
                // Send framebuffer updates
                frame_result = rx.recv() => {
                    match frame_result {
                        Ok(frame_data) => {
                            if let Err(e) = Self::send_framebuffer_update(stream, &frame_data).await {
                                eprintln!("Failed to send framebuffer update: {}", e);
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                
                // Handle client messages
                read_result = stream.read(&mut buffer) => {
                    match read_result {
                        Ok(0) => break, // Connection closed
                        Ok(n) => {
                            if let Err(e) = Self::process_vnc_message(&buffer[..n], stream, &hid_manager).await {
                                eprintln!("VNC message processing error: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("VNC read error: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn process_vnc_message(
        data: &[u8],
        _stream: &mut tokio::net::TcpStream,
        hid_manager: &HidManager,
    ) -> anyhow::Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        match data[0] {
            0 => { // SetPixelFormat
                println!("Received SetPixelFormat message");
            }
            2 => { // SetEncodings
                println!("Received SetEncodings message");
            }
            3 => { // FramebufferUpdateRequest
                println!("Received FramebufferUpdateRequest");
            }
            4 => { // KeyEvent
                if data.len() >= 8 {
                    let down_flag = data[1] != 0;
                    let key = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                    
                    println!("Key event: key={}, down={}", key, down_flag);
                    
                    if let Some(hid_report) = Self::vnc_key_to_hid(key, down_flag) {
                        let _ = hid_manager.send_keyboard_input(&hid_report).await;
                    }
                }
            }
            5 => { // PointerEvent
                if data.len() >= 6 {
                    let button_mask = data[1];
                    let x = u16::from_be_bytes([data[2], data[3]]);
                    let y = u16::from_be_bytes([data[4], data[5]]);
                    
                    println!("Pointer event: buttons={}, x={}, y={}", button_mask, x, y);
                    
                    let hid_report = Self::vnc_pointer_to_hid(button_mask, x, y);
                    let _ = hid_manager.send_mouse_input(&hid_report).await;
                }
            }
            6 => { // ClientCutText
                println!("Received ClientCutText message");
            }
            _ => {
                println!("Unknown VNC message type: {}", data[0]);
            }
        }
        
        Ok(())
    }

    async fn send_framebuffer_update(
        stream: &mut tokio::net::TcpStream,
        frame_data: &[u8],
    ) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;

        // FramebufferUpdate message
        let mut update = Vec::new();
        update.push(0); // message type
        update.push(0); // padding
        update.extend_from_slice(&1u16.to_be_bytes()); // number of rectangles

        // Rectangle header
        update.extend_from_slice(&0u16.to_be_bytes()); // x
        update.extend_from_slice(&0u16.to_be_bytes()); // y
        update.extend_from_slice(&1920u16.to_be_bytes()); // width
        update.extend_from_slice(&1080u16.to_be_bytes()); // height
        update.extend_from_slice(&0u32.to_be_bytes()); // encoding (Raw)

        stream.write_all(&update).await?;
        stream.write_all(frame_data).await?;
        stream.flush().await?;

        Ok(())
    }

    fn vnc_key_to_hid(vnc_key: u32, down: bool) -> Option<[u8; 8]> {
        // Basic VNC to HID keyboard mapping
        // This is a simplified mapping - you'd want a complete translation table
        let hid_key = match vnc_key {
            0xff08 => 0x2a, // Backspace
            0xff09 => 0x2b, // Tab
            0xff0d => 0x28, // Enter
            0xff1b => 0x29, // Escape
            0xff50 => 0x4f, // Home
            0xff51 => 0x50, // Left arrow
            0xff52 => 0x52, // Up arrow
            0xff53 => 0x4f, // Right arrow
            0xff54 => 0x51, // Down arrow
            0x0020 => 0x2c, // Space
            0x0041..=0x005a => (vnc_key - 0x0041 + 0x04) as u8, // A-Z
            0x0061..=0x007a => (vnc_key - 0x0061 + 0x04) as u8, // a-z
            0x0030..=0x0039 => (vnc_key - 0x0030 + 0x27) as u8, // 0-9
            _ => return None,
        };

        if down {
            Some([0, 0, hid_key, 0, 0, 0, 0, 0])
        } else {
            Some([0, 0, 0, 0, 0, 0, 0, 0]) // Key release
        }
    }

    fn vnc_pointer_to_hid(button_mask: u8, _x: u16, _y: u16) -> [u8; 4] {
        // Basic VNC to HID mouse mapping
        let buttons = button_mask & 0x07; // Left, middle, right buttons
        
        // For simplicity, we're not doing relative movement calculation here
        // In a real implementation, you'd calculate dx/dy from previous position
        let dx = 0i8; // Relative X movement
        let dy = 0i8; // Relative Y movement
        
        [buttons, dx as u8, dy as u8, 0]
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
    println!("  WebSocket listening on: {}:{}", args.bind_address, args.port);
    println!("  VNC listening on: {}:{}", args.bind_address, args.vnc_port);

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

    // 4. VNC server
    let vnc_handler = VncHandler::new(hub.clone(), hid_manager.clone());
    let vnc_bind_addr = args.bind_address.clone();
    let vnc_port = args.vnc_port;
    tokio::spawn(async move {
        if let Err(e) = vnc_handler.start_vnc_server(vnc_bind_addr, vnc_port).await {
            eprintln!("VNC server error: {}", e);
        }
    });

    // 5. Servidor HTTP → WS
    let app = Router::new()
        .route("/kvm/0", get({
            let h = hub.clone();
            let hid_mgr = hid_manager.clone();
            move |ws| kvm_ws(ws, h, hid_mgr)
        }));

    println!("KVM‑RS WebSocket listening on {}:{}", args.bind_address, args.port);
    
    // Create TCP listener with configurable address and port
    let bind_addr = format!("{}:{}", args.bind_address, args.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await
        .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", bind_addr, e))?;
    
    // Start the server using axum::serve
    axum::serve(listener, app).await?;

    Ok(())
}
