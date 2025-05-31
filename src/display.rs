// SPDX-License-Identifier: Apache-2.0
//
// Display hub with V4L2 and framebuffer support for kvm-rs

use std::sync::Arc;
use tokio::sync::broadcast;
use anyhow::Result;

/// Video capture mode detected or forced
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used on Linux only
pub enum CaptureMode {
    V4L2,
    Framebuffer,
    Mock,
}

/// Shared video frame broadcaster
pub struct DisplayHub {
    pub tx: broadcast::Sender<Vec<u8>>,
}

impl DisplayHub {
    pub fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(16);
        Arc::new(Self { tx })
    }

    #[cfg(target_os = "linux")]
    fn get_device_index_from_path(path: &str) -> usize {
        // Extract device number from paths like "/dev/video0", "/dev/video1", etc.
        if path.starts_with("/dev/video") {
            // Try to extract the number after "video"
            let number_part = path.trim_start_matches("/dev/video");
            number_part.parse().unwrap_or(0)
        } else {
            // Default to device 0 if we can't parse
            0
        }
    }

    pub async fn spawn(self: Arc<Self>, video_device_path: String, force_framebuffer: bool) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mode = if force_framebuffer {
                CaptureMode::Framebuffer
            } else {
                self.detect_capture_mode(&video_device_path).await
            };
            
            println!("Using capture mode: {:?}", mode);
            
            match mode {
                CaptureMode::V4L2 => self.spawn_v4l2_capture(video_device_path).await,
                CaptureMode::Framebuffer => self.spawn_framebuffer_capture(video_device_path).await,
                CaptureMode::Mock => self.spawn_mock_capture(video_device_path).await,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = force_framebuffer; // Suppress unused warning
            self.spawn_mock_capture(video_device_path).await
        }
    }

    #[cfg(target_os = "linux")]
    async fn detect_capture_mode(&self, video_device_path: &str) -> CaptureMode {
        use std::path::Path;
        
        // Check if it's a V4L2 device first
        if video_device_path.starts_with("/dev/video") {
            if Path::new(video_device_path).exists() {
                // Try to open as V4L2 device
                let device_index = Self::get_device_index_from_path(video_device_path);
                if let Ok(_) = v4l::Device::new(device_index) {
                    return CaptureMode::V4L2;
                }
            }
            println!("Warning: V4L2 device {} not available, trying framebuffer fallback", video_device_path);
            
            // Fallback to common framebuffer devices
            for fb_path in ["/dev/fb0", "/dev/fb1"] {
                if Path::new(fb_path).exists() {
                    println!("Found framebuffer device: {}", fb_path);
                    return CaptureMode::Framebuffer;
                }
            }
        }
        
        // Check if it's a framebuffer device
        if video_device_path.starts_with("/dev/fb") {
            if Path::new(video_device_path).exists() {
                return CaptureMode::Framebuffer;
            }
        }
        
        // If the specified path exists, try to determine type
        if Path::new(video_device_path).exists() {
            // If it contains "video", assume V4L2
            if video_device_path.contains("video") {
                let device_index = Self::get_device_index_from_path(video_device_path);
                if let Ok(_) = v4l::Device::new(device_index) {
                    return CaptureMode::V4L2;
                }
            }
            // If it contains "fb", assume framebuffer
            if video_device_path.contains("fb") {
                return CaptureMode::Framebuffer;
            }
            // Default to framebuffer for unknown device types
            return CaptureMode::Framebuffer;
        }
        
        println!("Warning: No video device found, using mock capture");
        CaptureMode::Mock
    }

    #[cfg(target_os = "linux")]
    async fn spawn_v4l2_capture(self: Arc<Self>, video_device_path: String) -> Result<()> {
        use v4l::Device;
        use anyhow::Context;

        println!("Starting V4L2 capture from: {}", video_device_path);

        // Open V4L2 device
        let device_index = Self::get_device_index_from_path(&video_device_path);
        let dev = Device::new(device_index)
            .with_context(|| format!("Failed to open V4L2 device: {} (index: {})", video_device_path, device_index))?;

        println!("Opened V4L2 device: {}", video_device_path);

        // Get device capabilities
        let caps = dev.query_caps()
            .context("Failed to query device capabilities")?;
        
        println!("Device capabilities: {}", caps);

        if caps.to_string().contains("Thumbnail") {
            println!("Detected thumbnail/snapshot device, using read-based capture");
            self.spawn_v4l2_read_capture(dev, video_device_path).await
        } else {
            println!("Getting current format for streaming device...");
            
            // Get current format for streaming devices
            let fmt = match v4l::video::Capture::format(&dev) {
                Ok(current_fmt) => {
                    println!("Current format: {:?} {}x{}", 
                        std::str::from_utf8(&current_fmt.fourcc.repr).unwrap_or("unknown"),
                        current_fmt.width, current_fmt.height);
                    current_fmt
                }
                Err(e) => {
                    println!("Failed to get current format: {}", e);
                    return Err(anyhow::anyhow!("Cannot get format from device: {}", e));
                }
            };

            println!("Using format: {:?} {}x{}", 
                std::str::from_utf8(&fmt.fourcc.repr).unwrap_or("unknown"),
                fmt.width, fmt.height);
            
            println!("Detected streaming device, invoking streaming capture method");
            self.spawn_v4l2_streaming_capture(dev, fmt).await
        }
    }

    #[cfg(target_os = "linux")]
    async fn spawn_v4l2_streaming_capture(self: Arc<Self>, dev: v4l::Device, fmt: v4l::Format) -> Result<()> {
        use v4l::{buffer::Type, io::traits::CaptureStream};
        use v4l::prelude::MmapStream;
        use anyhow::Context;

        // Create capture stream
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, 4)
            .context("Failed to create mmap stream")?;

        println!("Started V4L2 streaming capture");

        let mut frame_counter = 0u32;
        let mut last_successful_frame: Option<Vec<u8>> = None;

        loop {
            match stream.next() {
                Ok((buf, meta)) => {
                    // Convert frame data to Vec<u8> for broadcasting
                    let frame_data = match &fmt.fourcc.repr {
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

                    // Store successful frame
                    last_successful_frame = Some(frame_data.clone());

                    // Broadcast frame to all subscribers
                    let _ = self.tx.send(frame_data);

                    frame_counter += 1;
                    if frame_counter % 30 == 0 { // Every second at 30fps
                        println!("V4L2: Captured frame {}, size: {} bytes", meta.sequence, buf.len());
                    }
                }
                Err(e) => {
                    println!("V4L2 capture error: {}, retrying in 100ms...", e);
                    
                    // If we have a last successful frame, broadcast it to keep the stream alive
                    if let Some(ref frame_data) = last_successful_frame {
                        let _ = self.tx.send(frame_data.clone());
                    }
                    
                    // Wait before retrying
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    
                    // Try to recreate the stream if it failed
                    match MmapStream::with_buffers(&dev, Type::VideoCapture, 4) {
                        Ok(new_stream) => {
                            stream = new_stream;
                            println!("V4L2: Successfully recreated stream");
                        }
                        Err(stream_err) => {
                            println!("V4L2: Failed to recreate stream: {}", stream_err);
                            // Continue with the old stream and try again next iteration
                        }
                    }
                }
            }
            
            // Small delay to prevent excessive CPU usage
            tokio::time::sleep(std::time::Duration::from_millis(33)).await; // ~30 FPS
        }
    }

    #[cfg(target_os = "linux")]
    async fn spawn_v4l2_read_capture(self: Arc<Self>, dev: v4l::Device, video_device_path: String) -> Result<()> {
        use v4l::{buffer::Type, io::traits::CaptureStream};
        use v4l::prelude::MmapStream;

        println!("Started V4L2 read-based capture for snapshot device: {}", video_device_path);

        // Try to get device format, but don't fail if we can't
        if let Ok(fmt) = v4l::video::Capture::format(&dev) {
            println!("Snapshot device format: {:?} {}x{}", 
                std::str::from_utf8(&fmt.fourcc.repr).unwrap_or("unknown"),
                fmt.width, fmt.height);
        } else {
            println!("Warning: Could not get format from snapshot device, proceeding anyway");
        }

        let mut frame_counter = 0u32;
        let mut last_successful_frame: Option<Vec<u8>> = None;

        loop {
            // For snapshot devices, create a new stream for each capture attempt
            match MmapStream::with_buffers(&dev, Type::VideoCapture, 1) {
                Ok(mut stream) => {
                    match stream.next() {
                        Ok((buf, meta)) => {
                            println!("Snapshot: Captured frame {}, size: {} bytes", meta.sequence, buf.len());
                            
                            // Just use the raw buffer data
                            let frame_data = buf.to_vec();

                            // Store and broadcast the frame
                            last_successful_frame = Some(frame_data.clone());
                            match self.tx.send(frame_data) {
                                Ok(_) => {
                                    frame_counter += 1;
                                    if frame_counter % 10 == 0 {
                                        println!("Snapshot: Successfully captured and broadcasted frame {}", frame_counter);
                                    }
                                }
                                Err(e) => println!("Error broadcasting frame: {}", e),
                            }
                        }
                        Err(e) => {
                            println!("V4L2 snapshot capture error: {}", e);
                            // Broadcast last successful frame if available
                            if let Some(ref frame_data) = last_successful_frame {
                                let _ = self.tx.send(frame_data.clone());
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("Error creating snapshot stream: {}", e);
                    // Broadcast last successful frame if available
                    if let Some(ref frame_data) = last_successful_frame {
                        let _ = self.tx.send(frame_data.clone());
                    }
                }
            }

            // Wait between snapshot attempts - snapshot devices don't need high frequency
            tokio::time::sleep(std::time::Duration::from_millis(500)).await; // 2 FPS for snapshot
        }
    }

    #[cfg(target_os = "linux")]
    async fn spawn_framebuffer_capture(self: Arc<Self>, video_device_path: String) -> Result<()> {
        use tokio::{fs::File, io::AsyncReadExt};
        use anyhow::Context;

        println!("Starting framebuffer capture from: {}", video_device_path);

        // Try to determine framebuffer properties
        let (width, height, bpp) = self.get_framebuffer_info(&video_device_path).await
            .unwrap_or((1920, 1080, 4)); // Default to 1080p RGBA

        println!("Framebuffer: {}x{} @ {} bytes per pixel", width, height, bpp);
        
        let mut file = File::open(&video_device_path).await
            .with_context(|| format!("Failed to open framebuffer device: {}", video_device_path))?;
        
        let mut buf = vec![0u8; width * height * bpp];
        let mut frame_counter = 0u32;

        loop {
            // Read framebuffer data
            match file.read_exact(&mut buf).await {
                Ok(_) => {
                    // Broadcast frame to all subscribers
                    let _ = self.tx.send(buf.clone());
                    
                    frame_counter += 1;
                    if frame_counter % 300 == 0 { // Every 10 seconds at 30fps
                        println!("Framebuffer: Read frame {}, size: {} bytes", frame_counter, buf.len());
                    }
                }
                Err(e) => {
                    println!("Framebuffer read error: {}, retrying...", e);
                    // Try to reopen the file
                    match File::open(&video_device_path).await {
                        Ok(new_file) => file = new_file,
                        Err(reopen_err) => {
                            println!("Failed to reopen framebuffer: {}", reopen_err);
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }
                    }
                }
            }
            
            // ~30 FPS
            tokio::time::sleep(std::time::Duration::from_millis(33)).await;
        }
    }

    #[cfg(target_os = "linux")]
    async fn get_framebuffer_info(&self, fb_path: &str) -> Option<(usize, usize, usize)> {
        use std::fs;
        
        // Try to read framebuffer info from sysfs
        let fb_name = fb_path.trim_start_matches("/dev/");
        let info_paths = [
            format!("/sys/class/graphics/{}/virtual_size", fb_name),
            format!("/sys/class/graphics/{}/bits_per_pixel", fb_name),
        ];
        
        if let (Ok(size_str), Ok(bpp_str)) = (
            fs::read_to_string(&info_paths[0]),
            fs::read_to_string(&info_paths[1])
        ) {
            let size_parts: Vec<&str> = size_str.trim().split(',').collect();
            if size_parts.len() == 2 {
                if let (Ok(width), Ok(height), Ok(bpp)) = (
                    size_parts[0].parse::<usize>(),
                    size_parts[1].parse::<usize>(),
                    bpp_str.trim().parse::<usize>()
                ) {
                    let bytes_per_pixel = (bpp + 7) / 8; // Round up to nearest byte
                    println!("Detected framebuffer: {}x{} @ {} bpp ({} bytes/pixel)", 
                            width, height, bpp, bytes_per_pixel);
                    return Some((width, height, bytes_per_pixel));
                }
            }
        }
        
        println!("Could not detect framebuffer properties, using defaults");
        None
    }

    #[cfg(target_os = "linux")]
    async fn spawn_mock_capture(self: Arc<Self>, video_device_path: String) -> Result<()> {
        println!("Mock video capture on Linux (device: {})", video_device_path);
        println!("Note: Neither V4L2 nor framebuffer devices were available. Using mock implementation.");
        
        // Generate mock video data
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

    #[cfg(not(target_os = "linux"))]
    async fn spawn_mock_capture(self: Arc<Self>, video_device_path: String) -> Result<()> {
        println!("Mock video capture for development (device: {})", video_device_path);
        println!("Note: V4L2/Framebuffer capture only works on Linux. This is a mock implementation for development.");
        
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

    #[cfg(target_os = "linux")]
    #[allow(dead_code)] // May be used for different types of thumbnail devices
    async fn spawn_v4l2_snapshot_capture(self: Arc<Self>, dev: v4l::Device, fmt: v4l::Format) -> Result<()> {
        use v4l::{buffer::Type, io::traits::CaptureStream};
        use v4l::prelude::MmapStream;

        println!("Started V4L2 snapshot capture for thumbnail device");

        let mut frame_counter = 0u32;
        let mut last_successful_frame: Option<Vec<u8>> = None;

        loop {
            println!("Attempting to create a new stream for snapshot capture...");
            match MmapStream::with_buffers(&dev, Type::VideoCapture, 1) {
                Ok(mut stream) => {
                    println!("Stream created successfully, attempting to capture frame...");
                    match stream.next() {
                        Ok((buf, _meta)) => {
                            println!("Frame captured successfully, size: {} bytes", buf.len());
                            let frame_data = match &fmt.fourcc.repr {
                                b"MJPG" => buf.to_vec(),
                                b"YUYV" => buf.to_vec(),
                                _ => buf.to_vec(),
                            };

                            last_successful_frame = Some(frame_data.clone());
                            let broadcast_result = self.tx.send(frame_data);
                            match broadcast_result {
                                Ok(_) => println!("Frame broadcasted successfully"),
                                Err(e) => println!("Error broadcasting frame: {}", e),
                            }
                            frame_counter += 1;
                            if frame_counter % 30 == 0 {
                                println!("Snapshot: Captured frame {}, size: {} bytes", frame_counter, buf.len());
                            }
                        }
                        Err(e) => {
                            println!("Error capturing frame: {}", e);
                            if let Some(ref frame_data) = last_successful_frame {
                                let broadcast_result = self.tx.send(frame_data.clone());
                                match broadcast_result {
                                    Ok(_) => println!("Last successful frame broadcasted successfully"),
                                    Err(e) => println!("Error broadcasting last successful frame: {}", e),
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("Error creating stream: {}", e);
                    println!("Device may have stopped. Retrying in 1 second...");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            }

            // Wait between snapshot attempts
            tokio::time::sleep(std::time::Duration::from_millis(100)).await; // 10 FPS for snapshot
        }
    }
}
