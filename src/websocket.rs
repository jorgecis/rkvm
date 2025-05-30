// SPDX-License-Identifier: Apache-2.0
//
// WebSocket handler for kvm-rs

use std::sync::Arc;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
};
use crate::{display::DisplayHub, hid::HidManager};

/// WebSocket handler for KVM over WebSocket connections
pub async fn kvm_ws(
    ws: WebSocketUpgrade,
    hub: Arc<DisplayHub>,
    hid_manager: HidManager,
) -> Response {
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
                            if !data.is_empty() {
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
