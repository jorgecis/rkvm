// SPDX-License-Identifier: Apache-2.0
//
// kvm-rs: Minimal KVM‑IP server for OpenBMC
//
// Build: cargo build --release --target armv7-unknown-linux-gnueabihf
// Run  : systemd unit (ver §4)

mod args;
mod display;
mod hid;
mod vnc;
mod websocket;

use axum::{routing::get, Router};
use clap::Parser;
use zbus::Connection;

use args::Args;
use display::DisplayHub;
use hid::HidManager;
use vnc::VncHandler;
use websocket::kvm_ws;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Print configuration and validate devices
    args.print_config();
    args.validate_devices();

    // 1. Conecta a DBus para verificar sesión válida (Redfish)
    let _dbus: Connection = Connection::system().await?;

    // 2. Framebuffer broadcaster
    let hub = DisplayHub::new();
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
