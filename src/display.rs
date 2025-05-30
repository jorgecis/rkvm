// SPDX-License-Identifier: Apache-2.0
//
// Display hub and framebuffer management for kvm-rs

use std::sync::Arc;
use tokio::{fs::File, io::AsyncReadExt, sync::broadcast};

/// Shared framebuffer broadcaster
pub struct DisplayHub {
    pub tx: broadcast::Sender<Vec<u8>>,
}

impl DisplayHub {
    pub fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(16);
        Arc::new(Self { tx })
    }

    pub async fn spawn(self: Arc<Self>, fb_path: String) -> anyhow::Result<()> {
        let mut file = File::open(fb_path).await?;
        let mut buf = vec![0u8; 1920 * 1080 * 4]; // 1080p RGBA

        loop {
            file.read_exact(&mut buf).await?; // read raw frame
            let _ = self.tx.send(buf.clone()); // broadcast
            tokio::time::sleep(std::time::Duration::from_millis(33)).await; // ~30 fps
        }
    }
}
