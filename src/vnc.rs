// SPDX-License-Identifier: Apache-2.0
//
// VNC server implementation for kvm-rs with TLS encryption support

use std::sync::Arc;
use crate::{display::DisplayHub, hid::HidManager};
use anyhow::{Result, Context};

/// VNC Server handler for noVNC clients with TLS encryption
pub struct VncHandler {
    hub: Arc<DisplayHub>,
    hid_manager: HidManager,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
}

impl VncHandler {
    pub fn new(hub: Arc<DisplayHub>, hid_manager: HidManager) -> Self {
        Self {
            hub,
            hid_manager,
            tls_acceptor: None,
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
        
        let listener = TcpListener::bind(format!("{}:{}", bind_addr, port)).await
            .with_context(|| format!("Failed to bind VNC server to {}:{}", bind_addr, port))?;
        
        if self.tls_acceptor.is_some() {
            println!("VNC server with TLS encryption listening on {}:{}", bind_addr, port);
        } else {
            println!("VNC server (unencrypted) listening on {}:{}", bind_addr, port);
        }

        while let Ok((stream, addr)) = listener.accept().await {
            println!("VNC client connected from: {}", addr);
            
            let hub = self.hub.clone();
            let hid_manager = self.hid_manager.clone();
            let tls_acceptor = self.tls_acceptor.clone();
            
            tokio::spawn(async move {
                let result = if let Some(tls_acceptor) = tls_acceptor {
                    // Handle TLS connection
                    match tls_acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            Self::handle_vnc_client_tls(tls_stream, hub, hid_manager).await
                        }
                        Err(e) => {
                            eprintln!("TLS handshake failed for {}: {}", addr, e);
                            return;
                        }
                    }
                } else {
                    // Handle plain TCP connection
                    Self::handle_vnc_client_tcp(stream, hub, hid_manager).await
                };

                if let Err(e) = result {
                    eprintln!("VNC client error for {}: {}", addr, e);
                }
            });
        }
        
        Ok(())
    }

    async fn handle_vnc_client_tls(
        mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        hub: Arc<DisplayHub>,
        hid_manager: HidManager,
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
        let server_init = Self::create_server_init();
        stream.write_all(&server_init).await?;

        // Start framebuffer updates and input handling
        Self::handle_vnc_session_tls(stream, hub, hid_manager).await?;

        Ok(())
    }

    async fn handle_vnc_client_tcp(
        mut stream: tokio::net::TcpStream,
        hub: Arc<DisplayHub>,
        hid_manager: HidManager,
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
        let server_init = Self::create_server_init();
        stream.write_all(&server_init).await?;

        // Start framebuffer updates and input handling
        Self::handle_vnc_session_tcp(stream, hub, hid_manager).await?;

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

    async fn handle_vnc_session_tls(
        mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        hub: Arc<DisplayHub>,
        hid_manager: HidManager,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        
        let mut rx = hub.tx.subscribe();
        let mut buffer = [0u8; 1024];
        
        loop {
            tokio::select! {
                // Send framebuffer updates
                frame_result = rx.recv() => {
                    match frame_result {
                        Ok(frame_data) => {
                            if let Err(e) = Self::send_framebuffer_update_tls(&mut stream, &frame_data).await {
                                eprintln!("Failed to send framebuffer update (TLS): {}", e);
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
                            if let Err(e) = Self::process_vnc_message(&buffer[..n], &hid_manager).await {
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
        mut stream: tokio::net::TcpStream,
        hub: Arc<DisplayHub>,
        hid_manager: HidManager,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        
        let mut rx = hub.tx.subscribe();
        let mut buffer = [0u8; 1024];
        
        loop {
            tokio::select! {
                // Send framebuffer updates
                frame_result = rx.recv() => {
                    match frame_result {
                        Ok(frame_data) => {
                            if let Err(e) = Self::send_framebuffer_update_tcp(&mut stream, &frame_data).await {
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
                            if let Err(e) = Self::process_vnc_message(&buffer[..n], &hid_manager).await {
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
        hid_manager: &HidManager,
    ) -> Result<()> {
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

    async fn send_framebuffer_update_tls(
        stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        frame_data: &[u8],
    ) -> Result<()> {
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

    async fn send_framebuffer_update_tcp(
        stream: &mut tokio::net::TcpStream,
        frame_data: &[u8],
    ) -> Result<()> {
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
