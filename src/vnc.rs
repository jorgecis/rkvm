// SPDX-License-Identifier: Apache-2.0
//
// VNC server implementation for kvm-rs

use std::sync::Arc;
use crate::{display::DisplayHub, hid::HidManager};

/// VNC Server handler for noVNC clients
pub struct VncHandler {
    hub: Arc<DisplayHub>,
    hid_manager: HidManager,
}

impl VncHandler {
    pub fn new(hub: Arc<DisplayHub>, hid_manager: HidManager) -> Self {
        Self {
            hub,
            hid_manager,
        }
    }

    pub async fn start_vnc_server(self, bind_addr: String, port: u16) -> anyhow::Result<()> {
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
