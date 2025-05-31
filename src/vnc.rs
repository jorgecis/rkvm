// SPDX-License-Identifier: Apache-2.0
//
// VNC server implementation for kvm-rs with TLS encryption support

use std::sync::Arc;
use tokio::sync::RwLock;
use crate::{display::DisplayHub, hid::HidManager};
use anyhow::{Result, Context};

/// VNC Server handler for noVNC clients with TLS encryption
#[derive(Clone)]
pub struct VncHandler {
    hub: Arc<DisplayHub>,
    hid_manager: HidManager,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    last_frame: Arc<RwLock<Option<Vec<u8>>>>,
    frame_width: Arc<RwLock<u16>>,
    frame_height: Arc<RwLock<u16>>,
}

impl VncHandler {
    pub fn new(hub: Arc<DisplayHub>, hid_manager: HidManager) -> Self {
        Self {
            hub,
            hid_manager,
            tls_acceptor: None,
            last_frame: Arc::new(RwLock::new(None)),
            frame_width: Arc::new(RwLock::new(1920)),
            frame_height: Arc::new(RwLock::new(1080)),
        }
    }

    pub async fn new_with_tls(hub: Arc<DisplayHub>, hid_manager: HidManager, cert_path: Option<String>, key_path: Option<String>) -> Result<Self> {
        let tls_acceptor = if let (Some(cert), Some(key)) = (cert_path, key_path) {
            Some(Self::create_tls_acceptor(&cert, &key).await?)
        } else {
            // Generate self-signed certificate if no paths provided
            Some(Self::create_self_signed_tls_acceptor().await?)
        };

        Ok(Self {
            hub,
            hid_manager,
            tls_acceptor,
            last_frame: Arc::new(RwLock::new(None)),
            frame_width: Arc::new(RwLock::new(1920)),
            frame_height: Arc::new(RwLock::new(1080)),
        })
    }

    async fn create_tls_acceptor(cert_path: &str, key_path: &str) -> Result<tokio_rustls::TlsAcceptor> {
        use tokio::fs;
        use rustls::ServerConfig;
        use rustls_pemfile::{certs, private_key};
        use std::io::Cursor;

        // Read certificate file
        let cert_data = fs::read(cert_path).await
            .with_context(|| format!("Failed to read certificate file: {}", cert_path))?;
        
        // Read private key file
        let key_data = fs::read(key_path).await
            .with_context(|| format!("Failed to read private key file: {}", key_path))?;

        // Parse certificates
        let cert_chain = certs(&mut Cursor::new(&cert_data))
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse certificate chain")?;

        // Parse private key
        let private_key = private_key(&mut Cursor::new(&key_data))
            .context("Failed to parse private key")?
            .ok_or_else(|| anyhow::anyhow!("No private key found in key file"))?;

        // Create TLS config
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)
            .context("Failed to create TLS configuration")?;

        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }

    async fn create_self_signed_tls_acceptor() -> Result<tokio_rustls::TlsAcceptor> {
        use rustls::ServerConfig;
        use rcgen::{CertificateParams, DistinguishedName, KeyPair};
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        println!("Generating self-signed certificate for VNC TLS...");

        // Generate key pair
        let key_pair = KeyPair::generate()
            .context("Failed to generate key pair")?;

        // Generate self-signed certificate
        let mut params = CertificateParams::new(vec!["localhost".to_string()])?;
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "KVM-RS VNC Server");
        dn.push(rcgen::DnType::OrganizationName, "OpenBMC");
        params.distinguished_name = dn;
        
        let cert = params.self_signed(&key_pair)
            .context("Failed to generate self-signed certificate")?;

        // Convert to rustls format  
        let cert_der = CertificateDer::from(cert.der().clone());
        let key_der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

        // Create TLS config
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .context("Failed to create TLS configuration with self-signed certificate")?;

        println!("Self-signed certificate generated successfully");
        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }

    pub async fn start_vnc_server(self, bind_addr: String, port: u16) -> Result<()> {
        use tokio::net::TcpListener;
        
        // Start frame processing task
        let frame_processor = self.clone();
        tokio::spawn(async move {
            frame_processor.process_frames().await;
        });
        
        let listener = TcpListener::bind(format!("{}:{}", bind_addr, port)).await
            .with_context(|| format!("Failed to bind VNC server to {}:{}", bind_addr, port))?;
        
        if self.tls_acceptor.is_some() {
            println!("VNC server with TLS encryption listening on {}:{}", bind_addr, port);
        } else {
            println!("VNC server (unencrypted) listening on {}:{}", bind_addr, port);
        }

        while let Ok((stream, addr)) = listener.accept().await {
            println!("VNC client connected from: {}", addr);
            
            let handler = self.clone();
            
            tokio::spawn(async move {
                let result = if let Some(ref tls_acceptor) = handler.tls_acceptor {
                    // Handle TLS connection
                    match tls_acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            handler.handle_vnc_client_tls(tls_stream).await
                        }
                        Err(e) => {
                            eprintln!("TLS handshake failed for {}: {}", addr, e);
                            return;
                        }
                    }
                } else {
                    // Handle plain TCP connection
                    handler.handle_vnc_client_tcp(stream).await
                };

                if let Err(e) = result {
                    eprintln!("VNC client error for {}: {}", addr, e);
                }
            });
        }
        
        Ok(())
    }

    async fn process_frames(&self) {
        let mut rx = self.hub.tx.subscribe();
        
        while let Ok(frame_data) = rx.recv().await {
            // Convert frame data to RGB format for VNC
            let rgb_data = self.convert_frame_to_rgb(&frame_data).await;
            
            // Update last frame
            *self.last_frame.write().await = Some(rgb_data);
            
            // Update frame dimensions if needed (detect from frame data)
            // For now, assume the frame is already in the right format
        }
    }

    async fn convert_frame_to_rgb(&self, frame_data: &[u8]) -> Vec<u8> {
        // Try to detect frame format and convert to RGB
        // For now, assume it's already RGB or MJPEG
        
        // Check if it looks like MJPEG (starts with FF D8)
        if frame_data.len() > 2 && frame_data[0] == 0xFF && frame_data[1] == 0xD8 {
            // MJPEG data - decode to RGB
            if let Ok(img) = image::load_from_memory_with_format(frame_data, image::ImageFormat::Jpeg) {
                let rgb_img = img.to_rgb8();
                let (width, height) = rgb_img.dimensions();
                
                // Update dimensions
                *self.frame_width.write().await = width as u16;
                *self.frame_height.write().await = height as u16;
                
                println!("Decoded MJPEG frame: {}x{}", width, height);
                return rgb_img.into_raw();
            }
        }
        
        // Check if it might be YUYV (specific size patterns)
        let pixel_count = frame_data.len() / 2; // YUYV is 2 bytes per pixel
        let width_height_pairs = [
            (1920, 1080), (1280, 720), (640, 480), (320, 240)
        ];
        
        for (w, h) in width_height_pairs {
            if pixel_count == w * h {
                // Looks like YUYV with these dimensions
                println!("Converting YUYV frame: {}x{}", w, h);
                *self.frame_width.write().await = w as u16;
                *self.frame_height.write().await = h as u16;
                return self.convert_yuyv_to_rgb(frame_data, w, h);
            }
        }
        
        // Check if it might be RGB (3 bytes per pixel)
        let rgb_pixel_count = frame_data.len() / 3;
        for (w, h) in width_height_pairs {
            if rgb_pixel_count == w * h {
                // Already RGB
                println!("Using RGB frame: {}x{}", w, h);
                *self.frame_width.write().await = w as u16;
                *self.frame_height.write().await = h as u16;
                return frame_data.to_vec();
            }
        }
        
        // Default: assume it's RGB data, use default dimensions
        frame_data.to_vec()
    }

    fn convert_yuyv_to_rgb(&self, yuyv_data: &[u8], width: usize, height: usize) -> Vec<u8> {
        let mut rgb_data = Vec::with_capacity(width * height * 3);
        
        for chunk in yuyv_data.chunks_exact(4) {
            let y1 = chunk[0] as i32;
            let u = chunk[1] as i32 - 128;
            let y2 = chunk[2] as i32;
            let v = chunk[3] as i32 - 128;
            
            // Convert first pixel (Y1, U, V)
            let r1 = ((y1 + (1.402 * v as f32) as i32).max(0).min(255)) as u8;
            let g1 = ((y1 - (0.344 * u as f32) as i32 - (0.714 * v as f32) as i32).max(0).min(255)) as u8;
            let b1 = ((y1 + (1.772 * u as f32) as i32).max(0).min(255)) as u8;
            
            // Convert second pixel (Y2, U, V)
            let r2 = ((y2 + (1.402 * v as f32) as i32).max(0).min(255)) as u8;
            let g2 = ((y2 - (0.344 * u as f32) as i32 - (0.714 * v as f32) as i32).max(0).min(255)) as u8;
            let b2 = ((y2 + (1.772 * u as f32) as i32).max(0).min(255)) as u8;
            
            rgb_data.extend_from_slice(&[r1, g1, b1, r2, g2, b2]);
        }
        
        rgb_data
    }

    async fn handle_vnc_client_tls(
        &self,
        mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // VNC handshake over TLS
        // Send RFB protocol version
        stream.write_all(b"RFB 003.008\n").await?;
        
        // Read client protocol version
        let mut version_buf = [0u8; 12];
        stream.read_exact(&mut version_buf).await?;
        println!("Client VNC version (TLS): {}", String::from_utf8_lossy(&version_buf));

        // Security handshake - TLS security type
        stream.write_all(&[1u8, 18u8]).await?; // 1 security type: TLS (type 18)
        let mut security_choice = [0u8; 1];
        stream.read_exact(&mut security_choice).await?;
        
        if security_choice[0] != 18 {
            return Err(anyhow::anyhow!("Client chose unsupported security type for TLS connection"));
        }

        // Security result - OK
        stream.write_all(&[0u8, 0u8, 0u8, 0u8]).await?;

        // Read ClientInit
        let mut client_init = [0u8; 1];
        stream.read_exact(&mut client_init).await?;

        // Send ServerInit
        let server_init = self.create_server_init().await;
        stream.write_all(&server_init).await?;

        // Start framebuffer updates and input handling
        self.handle_vnc_session_tls(stream).await?;

        Ok(())
    }

    async fn handle_vnc_client_tcp(
        &self,
        mut stream: tokio::net::TcpStream,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // VNC handshake over plain TCP
        // Send RFB protocol version
        stream.write_all(b"RFB 003.008\n").await?;
        
        // Read client protocol version
        let mut version_buf = [0u8; 12];
        stream.read_exact(&mut version_buf).await?;
        println!("Client VNC version: {}", String::from_utf8_lossy(&version_buf));

        // Security handshake - no authentication for plain connections
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
        let server_init = self.create_server_init().await;
        stream.write_all(&server_init).await?;

        // Start framebuffer updates and input handling
        self.handle_vnc_session_tcp(stream).await?;

        Ok(())
    }

    async fn create_server_init(&self) -> Vec<u8> {
        let width = *self.frame_width.read().await;
        let height = *self.frame_height.read().await;
        
        let mut init = Vec::new();
        
        // Framebuffer width - big endian
        init.extend_from_slice(&width.to_be_bytes());
        // Framebuffer height - big endian  
        init.extend_from_slice(&height.to_be_bytes());
        
        // Pixel format (24-bit RGB)
        init.push(24); // bits per pixel
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
        
        println!("Sent ServerInit: {}x{} RGB24", width, height);
        init
    }

    async fn handle_vnc_session_tls(
        &self,
        mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        
        let mut rx = self.hub.tx.subscribe();
        let mut buffer = [0u8; 1024];
        
        loop {
            tokio::select! {
                // Send framebuffer updates when new frames arrive
                frame_result = rx.recv() => {
                    match frame_result {
                        Ok(_) => {
                            // Frame is already processed by process_frames task
                            if let Some(ref frame_data) = *self.last_frame.read().await {
                                if let Err(e) = self.send_framebuffer_update_tls(&mut stream, frame_data).await {
                                    eprintln!("Failed to send framebuffer update (TLS): {}", e);
                                    break;
                                }
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
                            if let Err(e) = self.process_vnc_message(&buffer[..n], &mut stream).await {
                                eprintln!("VNC message processing error (TLS): {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("VNC read error (TLS): {}", e);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_vnc_session_tcp(
        &self,
        mut stream: tokio::net::TcpStream,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        
        let mut rx = self.hub.tx.subscribe();
        let mut buffer = [0u8; 1024];
        
        loop {
            tokio::select! {
                // Send framebuffer updates when new frames arrive
                frame_result = rx.recv() => {
                    match frame_result {
                        Ok(_) => {
                            // Frame is already processed by process_frames task
                            if let Some(ref frame_data) = *self.last_frame.read().await {
                                if let Err(e) = self.send_framebuffer_update_tcp(&mut stream, frame_data).await {
                                    eprintln!("Failed to send framebuffer update: {}", e);
                                    break;
                                }
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
                            if let Err(e) = self.process_vnc_message(&buffer[..n], &mut stream).await {
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

    async fn process_vnc_message<S>(
        &self,
        data: &[u8],
        stream: &mut S,
    ) -> Result<()> 
    where
        S: tokio::io::AsyncWrite + Unpin,
    {
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
                
                // Send current framebuffer immediately if we have one
                if let Some(ref frame_data) = *self.last_frame.read().await {
                    use tokio::io::AsyncWriteExt;
                    
                    let width = *self.frame_width.read().await;
                    let height = *self.frame_height.read().await;
                    
                    // FramebufferUpdate message
                    let mut update = Vec::new();
                    update.push(0); // message type
                    update.push(0); // padding
                    update.extend_from_slice(&1u16.to_be_bytes()); // number of rectangles

                    // Rectangle header
                    update.extend_from_slice(&0u16.to_be_bytes()); // x
                    update.extend_from_slice(&0u16.to_be_bytes()); // y
                    update.extend_from_slice(&width.to_be_bytes()); // width
                    update.extend_from_slice(&height.to_be_bytes()); // height
                    update.extend_from_slice(&0u32.to_be_bytes()); // encoding (Raw)

                    stream.write_all(&update).await?;
                    stream.write_all(frame_data).await?;
                    stream.flush().await?;
                    
                    println!("Sent immediate framebuffer update: {}x{}, {} bytes", width, height, frame_data.len());
                }
            }
            4 => { // KeyEvent
                if data.len() >= 8 {
                    let down_flag = data[1] != 0;
                    let key = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                    
                    println!("Key event: key={}, down={}", key, down_flag);
                    
                    if let Some(hid_report) = Self::vnc_key_to_hid(key, down_flag) {
                        let _ = self.hid_manager.send_keyboard_input(&hid_report).await;
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
                    let _ = self.hid_manager.send_mouse_input(&hid_report).await;
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

    async fn send_framebuffer_update_tls(
        &self,
        stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        frame_data: &[u8],
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let width = *self.frame_width.read().await;
        let height = *self.frame_height.read().await;

        // FramebufferUpdate message
        let mut update = Vec::new();
        update.push(0); // message type
        update.push(0); // padding
        update.extend_from_slice(&1u16.to_be_bytes()); // number of rectangles

        // Rectangle header
        update.extend_from_slice(&0u16.to_be_bytes()); // x
        update.extend_from_slice(&0u16.to_be_bytes()); // y
        update.extend_from_slice(&width.to_be_bytes()); // width
        update.extend_from_slice(&height.to_be_bytes()); // height
        update.extend_from_slice(&0u32.to_be_bytes()); // encoding (Raw)

        stream.write_all(&update).await?;
        stream.write_all(frame_data).await?;
        stream.flush().await?;

        Ok(())
    }

    async fn send_framebuffer_update_tcp(
        &self,
        stream: &mut tokio::net::TcpStream,
        frame_data: &[u8],
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let width = *self.frame_width.read().await;
        let height = *self.frame_height.read().await;

        // FramebufferUpdate message
        let mut update = Vec::new();
        update.push(0); // message type
        update.push(0); // padding
        update.extend_from_slice(&1u16.to_be_bytes()); // number of rectangles

        // Rectangle header
        update.extend_from_slice(&0u16.to_be_bytes()); // x
        update.extend_from_slice(&0u16.to_be_bytes()); // y
        update.extend_from_slice(&width.to_be_bytes()); // width
        update.extend_from_slice(&height.to_be_bytes()); // height
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
