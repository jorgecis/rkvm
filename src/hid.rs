// SPDX-License-Identifier: Apache-2.0
//
// HID device management for kvm-rs

/// HID device manager for keyboard and mouse input
#[derive(Clone)]
pub struct HidManager {
    keyboard_device: String,
    mouse_device: String,
}

impl HidManager {
    pub fn new(keyboard_device: String, mouse_device: String) -> Self {
        Self {
            keyboard_device,
            mouse_device,
        }
    }

    /// Send keyboard input to HID gadget device
    pub async fn send_keyboard_input(&self, data: &[u8]) -> anyhow::Result<()> {
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
    pub async fn send_mouse_input(&self, data: &[u8]) -> anyhow::Result<()> {
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
