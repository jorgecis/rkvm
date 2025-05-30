# KVM-RS: Minimal KVM-IP Server for OpenBMC

A lightweight KVM over IP server designed for OpenBMC systems, providing remote console access via WebSocket connections.

## Features

- Framebuffer streaming from video devices
- HID gadget support for keyboard and mouse input
- WebSocket-based communication
- Configurable device paths and network settings
- DBus integration for session validation

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
| `--video <DEVICE>` | `-v` | `/dev/fb0` | Video device path (framebuffer) |
| `--keyboard-hid <DEVICE>` | `-k` | `/dev/hidg0` | HID gadget device for keyboard input |
| `--mouse-hid <DEVICE>` | `-m` | `/dev/hidg1` | HID gadget device for mouse input |
| `--port <PORT>` | `-p` | `8443` | Port to listen on |
| `--bind <ADDRESS>` | `-b` | `0.0.0.0` | Bind address |
| `--help` | `-h` | - | Print help information |

### Examples

```bash
# Run with default settings
kvm-rs

# Use custom video device and HID devices
kvm-rs --video /dev/fb1 --keyboard-hid /dev/hidg2 --mouse-hid /dev/hidg3

# Run on a different port and bind address
kvm-rs --port 9000 --bind 127.0.0.1

# Combine multiple options
kvm-rs -v /dev/fb0 -k /dev/hidg0 -m /dev/hidg1 -p 8443 -b 0.0.0.0
```

## WebSocket Endpoint

The server exposes a WebSocket endpoint at `/kvm/0` for KVM connections.

### Protocol

- **Video Output**: Framebuffer data is broadcast as binary messages to connected clients
- **Input Handling**: Binary messages from clients are interpreted as HID input:
  - Byte 0 = `0x01`: Keyboard input (remaining bytes sent to keyboard HID device)
  - Byte 0 = `0x02`: Mouse input (remaining bytes sent to mouse HID device)

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

## Development

The project uses:
- **axum** for HTTP/WebSocket server
- **tokio** for async runtime
- **zbus** for DBus communication
- **clap** for command-line parsing
- **broadcast** channels for framebuffer data distribution

## License

SPDX-License-Identifier: Apache-2.0
