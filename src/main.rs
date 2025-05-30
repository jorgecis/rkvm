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
use std::sync::Arc;
use tokio::{fs::File, io::AsyncReadExt, sync::broadcast};
use zbus::Connection;

/// Shared framebuffer broadcaster
struct DisplayHub {
    tx: broadcast::Sender<Vec<u8>>,
}

impl DisplayHub {
    async fn spawn(self: Arc<Self>, fb_path: &str) -> anyhow::Result<()> {
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
) -> axum::response::Response {
    ws.on_upgrade(|mut socket: WebSocket| async move {
        let mut rx = hub.tx.subscribe();
        // TODO: Handshake RFB / VNC here
        while let Ok(frame) = rx.recv().await {
            if socket.send(Message::Binary(frame.into())).await.is_err() {
                break;
            }
        }
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Conecta a DBus para verificar sesión válida (Redfish)
    let _dbus: Connection = Connection::system().await?;

    // 2. Framebuffer broadcaster
    let hub = Arc::new(DisplayHub {
        tx: broadcast::channel(16).0,
    });
    tokio::spawn(hub.clone().spawn("/dev/fb0"));

    // 3. Servidor HTTP → WS
    let app = Router::new()
        .route("/kvm/0", get({
            let h = hub.clone();
            move |ws| kvm_ws(ws, h)
        }));

    println!("KVM‑RS listening on 0.0.0.0:8443");
    
    // Create TCP listener
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8443").await?;
    
    // Start the server using axum::serve
    axum::serve(listener, app).await?;

    Ok(())
}
