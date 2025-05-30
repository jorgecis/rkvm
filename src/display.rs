// SPDX-License-Identifier: Apache-2.0
//
// Display hub and video capture management for kvm-rs

use std::sync::Arc;
use tokio::sync::broadcast;
use anyhow::Result;

/// Shared video frame broadcaster
pub struct DisplayHub {
    pub tx: broadcast::Sender<Vec<u8>>,
}

impl DisplayHub {
    pub fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(16);
        Arc::new(Self { tx })
    }

    pub async fn spawn(self: Arc<Self>, video_device_path: String) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.spawn_v4l2_capture(video_device_path).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.spawn_mock_capture(video_device_path).await
        }
    }

    #[cfg(target_os = "linux")]
    async fn spawn_v4l2_capture(self: Arc<Self>, video_device_path: String) -> Result<()> {
        use v4l::prelude::*;
        use v4l::{buffer::Type, io::traits::CaptureStream, Device, FourCC};
        use anyhow::Context;

        // Open V4L2 device
        let dev = Device::new(&video_device_path)
            .with_context(|| format!("Failed to open V4L2 device: {}", video_device_path))?;

        println!("Opened V4L2 device: {}", video_device_path);

        // Get device capabilities
        let caps = dev.query_caps()
            .context("Failed to query device capabilities")?;
        
        println!("Device capabilities: {}", caps);

        // Set format - try common formats
        let mut fmt = dev.format()
            .context("Failed to get current format")?;
        
        // Try to set a common format - MJPEG first, then YUYV
        fmt.fourcc = FourCC::new(b"MJPG");
        fmt.width = 1920;
        fmt.height = 1080;
        
        let fmt = match dev.set_format(&fmt) {
            Ok(f) => {
                println!("Set format to MJPEG {}x{}", f.width, f.height);
                f
            }
            Err(_) => {
                // Fallback to YUYV
                fmt.fourcc = FourCC::new(b"YUYV");
                fmt.width = 1920;
                fmt.height = 1080;
                let f = dev.set_format(&fmt)
                    .context("Failed to set video format (tried MJPEG and YUYV)")?;
                println!("Set format to YUYV {}x{}", f.width, f.height);
                f
            }
        };

        // Set frame rate
        let mut params = dev.params()
            .context("Failed to get stream parameters")?;
        params.interval = v4l::Fraction::new(1, 30); // 30 FPS
        dev.set_params(&params)
            .context("Failed to set frame rate")?;

        println!("Set frame rate to 30 FPS");

        // Create capture stream
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, 4)
            .context("Failed to create mmap stream")?;

        println!("Started V4L2 capture stream");

        loop {
            // Capture frame
            let (buf, meta) = stream.next()
                .context("Failed to capture frame")?;
            
            // Convert frame data to Vec<u8> for broadcasting
            let frame_data = match fmt.fourcc.repr {
                b"MJPG" => {
                    // MJPEG data can be used directly
                    buf.to_vec()
                }
                b"YUYV" => {
                    // For YUYV, we might want to convert to RGB or just pass raw
                    // For now, just pass the raw YUYV data
                    buf.to_vec()
                }
                _ => {
                    // For other formats, just pass raw data
                    buf.to_vec()
                }
            };

            // Broadcast frame to all subscribers
            let _ = self.tx.send(frame_data);

            // Print some debug info occasionally
            if meta.sequence % 300 == 0 { // Every 10 seconds at 30fps
                println!("Captured frame {}, size: {} bytes", meta.sequence, buf.len());
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    async fn spawn_mock_capture(self: Arc<Self>, video_device_path: String) -> Result<()> {
        println!("Mock video capture for development (device: {})", video_device_path);
        println!("Note: V4L2 capture only works on Linux. This is a mock implementation for development.");
        
        // Generate mock video data for development/testing
        let mut frame_counter = 0u32;
        loop {
            // Create a simple test pattern - alternating colors
            let color = match (frame_counter / 30) % 3 {
                0 => [255u8, 0, 0], // Red
                1 => [0u8, 255, 0], // Green
                _ => [0u8, 0, 255], // Blue
            };
            
            // Create a 640x480 RGB24 test frame
            let width = 640;
            let height = 480;
            let mut frame_data = Vec::with_capacity(width * height * 3);
            
            for y in 0..height {
                for x in 0..width {
                    // Create a simple pattern with the base color
                    let intensity = ((x + y + frame_counter as usize) % 256) as u8;
                    frame_data.push((color[0] as u16 * intensity as u16 / 255) as u8);
                    frame_data.push((color[1] as u16 * intensity as u16 / 255) as u8);
                    frame_data.push((color[2] as u16 * intensity as u16 / 255) as u8);
                }
            }

            // Broadcast mock frame
            let _ = self.tx.send(frame_data);

            frame_counter += 1;
            if frame_counter % 300 == 0 {
                println!("Mock capture: Generated frame {}", frame_counter);
            }

            // 30 FPS
            tokio::time::sleep(std::time::Duration::from_millis(33)).await;
        }
    }
}
