# KVM-RS: Minimal KVM-IP Server for OpenBMC

A lightweight KVM over IP server designed for OpenBMC systems, providing remote console access via WebSocket connections with support for both V4L2 video capture and framebuffer devices.

## Features

- **Dual video capture support**: V4L2 devices (USB cameras, HDMI capture cards) and framebuffer devices
- **Auto-detection**: Automatically detects the best available video source with intelligent fallback
- HID gadget support for keyboard and mouse input
- WebSocket-based communication for web clients
- **VNC server with TLS encryption support** for secure noVNC client connections
- **Configurable encryption**: Support for both encrypted (TLS) and unencrypted VNC connections
- **Self-signed certificates**: Automatic generation of TLS certificates or use custom certificates
- Configurable device paths and network settings
- DBus integration for session validation

## Video Source Support

### V4L2 Devices (Preferred)
- USB cameras (`/dev/video0`, `/dev/video1`, etc.)
- HDMI/DVI capture cards
- Other V4L2-compatible video sources
- Supports MJPEG and YUYV formats
- Automatic format detection and fallback

### Framebuffer Devices (Fallback)
- Direct framebuffer access (`/dev/fb0`, `/dev/fb1`, etc.)
- Automatic resolution and format detection
- Suitable for systems without V4L2 devices

## Build

```bash
# For local development
cargo build

# For ARM target (OpenBMC)
cargo build --release --target armv7-unknown-linux-gnueabihf
```

## Usage

```bash
kvm-rs [OPTIONS]
```

### Options

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `--video <DEVICE>` | `-v` | `/dev/video0` | Video device path (V4L2 or framebuffer) |
| `--force-framebuffer` | - | - | Force framebuffer mode, skip V4L2 detection |
| `--keyboard-hid <DEVICE>` | `-k` | `/dev/hidg0` | HID gadget device for keyboard input |
| `--mouse-hid <DEVICE>` | `-m` | `/dev/hidg1` | HID gadget device for mouse input |
| `--port <PORT>` | `-p` | `8443` | Port to listen on (WebSocket) |
| `--vnc-port <PORT>` | - | `5900` | VNC server port |
| `--vnc-tls` | - | - | Enable TLS encryption for VNC server |
| `--vnc-cert <FILE>` | - | - | TLS certificate file path (PEM format) |
| `--vnc-key <FILE>` | - | - | TLS private key file path (PEM format) |
| `--bind <ADDRESS>` | `-b` | `0.0.0.0` | Bind address |
| `--help` | `-h` | - | Print help information |

### Examples

```bash
# Run with default settings (auto-detect video source, WebSocket on 8443, VNC on 5900)
kvm-rs

# Use V4L2 video capture (USB camera/HDMI capture card)
kvm-rs --video /dev/video0

# Force framebuffer mode
kvm-rs --video /dev/fb0 --force-framebuffer

# Enable TLS encryption for VNC with auto-generated self-signed certificate
kvm-rs --vnc-tls

# Enable TLS encryption with custom certificate and key files
kvm-rs --vnc-tls --vnc-cert /path/to/cert.pem --vnc-key /path/to/key.pem

# Use custom devices
kvm-rs --video /dev/video1 --keyboard-hid /dev/hidg2 --mouse-hid /dev/hidg3

# Run on different ports with TLS
kvm-rs --port 9000 --vnc-port 5901 --vnc-tls

# Custom bind address with TLS encryption
kvm-rs --bind 127.0.0.1 --vnc-tls

# Combine multiple options with TLS
kvm-rs -v /dev/fb0 -k /dev/hidg0 -m /dev/hidg1 -p 8443 --vnc-port 5900 --vnc-tls -b 0.0.0.0
```

## WebSocket Endpoint

The server exposes a WebSocket endpoint at `/kvm/0` for KVM connections.

## VNC Server

The server also runs a VNC server on port 5900 (configurable with `--vnc-port`) that is compatible with noVNC clients.

### Protocol

#### WebSocket Protocol
- **Video Output**: Framebuffer data is broadcast as binary messages to connected clients
- **Input Handling**: Binary messages from clients are interpreted as HID input:
  - Byte 0 = `0x01`: Keyboard input (remaining bytes sent to keyboard HID device)
  - Byte 0 = `0x02`: Mouse input (remaining bytes sent to mouse HID device)

#### VNC Protocol
- **RFB 3.8**: Standard VNC protocol implementation
- **Security**: No authentication (for simplicity in OpenBMC environments)
- **Encoding**: Raw pixel format (32-bit RGBA, 1920x1080)
- **Input**: Standard VNC keyboard and pointer events converted to HID reports

## System Requirements

### HID Gadget Setup

For keyboard and mouse input to work, you need to configure USB HID gadgets:

```bash
# Create HID gadgets (usually done in OpenBMC initialization)
modprobe libcomposite
cd /sys/kernel/config/usb_gadget/
mkdir g1
cd g1
echo 0x1d6b > idVendor
echo 0x0104 > idProduct

# Create keyboard function
mkdir functions/hid.keyboard
echo 1 > functions/hid.keyboard/protocol
echo 1 > functions/hid.keyboard/subclass
echo 8 > functions/hid.keyboard/report_length

# Create mouse function  
mkdir functions/hid.mouse
echo 2 > functions/hid.mouse/protocol
echo 1 > functions/hid.mouse/subclass
echo 4 > functions/hid.mouse/report_length

# Enable the gadget
echo "udc_name" > UDC
```

### Framebuffer

Ensure the framebuffer device is accessible and provides the expected format (RGBA 1920x1080).

## Connecting with noVNC

You can connect to the KVM server using noVNC clients:

### Using noVNC Web Client

1. Deploy noVNC on a web server
2. Connect to: `vnc://your-openbmc-ip:5900`
3. No password required (authentication disabled for OpenBMC environments)

### Using VNC Viewer

Any standard VNC client can connect:
```bash
# Using vncviewer
vncviewer your-openbmc-ip:5900

# Using TigerVNC
vncviewer your-openbmc-ip::5900
```

### WebSocket Connection

For web-based clients, connect to:
```
ws://your-openbmc-ip:8443/kvm/0
```

## Development

The project uses:
- **axum** for HTTP/WebSocket server
- **tokio** for async runtime
- **zbus** for DBus communication
- **clap** for command-line parsing
- **broadcast** channels for framebuffer data distribution

## License

SPDX-License-Identifier: Apache-2.0
