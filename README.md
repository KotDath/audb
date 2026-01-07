# audb - Aurora Debug Bridge

Development and debugging CLI tool for Aurora OS, similar to Android's ADB.

> ⚠️ **Beta Software**
> 
> This project is in beta. It may contain bugs or unexpected behavior.
> If you encounter any issues, please report them in issues.

## Features

- **Device Management** - Add, remove, list, and select Aurora OS devices
- **Package Management** - Install, uninstall, sign, and validate RPM packages
- **Shell Access** - Execute commands on device (as user or root)
- **File Transfer** - Push/pull files via SFTP
- **Input Injection** - Tap, swipe, and key events with rotation support
- **Screenshots** - Capture device screen
- **App Control** - Launch and stop applications
- **Logs** - View and filter system logs
- **Device Info** - Get detailed hardware and software information

## Requirements

### Host Machine

| Requirement | Version | Notes |
|-------------|---------|-------|
| **Rust** | 1.70+ | For building from source |
| **Docker** | Any | Required for `package sign` and `package validate` |
| **Aurora Build Tools** | 5.2+ | Docker image for signing/validation |

### Target Device

| Requirement | Notes |
|-------------|-------|
| **Aurora OS** | SSH access enabled |
| **Python 3** | Required for `tap`, `swipe` commands |

Install Python on device:
```bash
devel-su pkcon install python3
```

### Aurora SDK Docker Image

For package signing and validation, you need the Aurora Build Tools Docker image.
Download from [Aurora OS Developer Portal](https://developer.auroraos.ru/).

Image names: `aurora-build-tools-*` or `aurora-os-build-engine-*`

### Signing Keys

Signing keys are automatically downloaded from Aurora OS developer portal on first use and cached in `~/.cache/audb/`.

To use custom keys:
```bash
audb package sign app.rpm --key /path/to/key.pem --cert /path/to/cert.pem
```

## Installation

### From crates.io

```bash
cargo install audb-client audb-server
```

This installs both binaries:
- `audb` - CLI client
- `audb-server` - Background server daemon

### From Source

```bash
git clone https://github.com/KotDath/audb
cd audb
cargo build --release
```

Binaries will be at:
- `target/release/audb` - CLI client
- `target/release/audb-server` - Background server

### Add to PATH (from source)

```bash
# Option 1: Copy to /usr/local/bin
sudo cp target/release/audb target/release/audb-server /usr/local/bin/

# Option 2: Add to PATH in ~/.bashrc
export PATH="$PATH:/path/to/audb/target/release"
```

## Quick Start

```bash
# 1. Add your device
audb device add

# 2. Select it as active
audb select 0

# 3. Test connection
audb ping

# 4. Run a command
audb shell uname -a
```

## Commands Reference

### Device Management

```bash
# List all devices
audb device list

# List only connected devices
audb device list --active

# Add new device interactively
audb device add

# Remove device (by index, IP, or name)
audb device remove 0
audb device remove 192.168.2.15
audb device remove my-device

# Select active device
audb select <identifier>
```

### Package Management

```bash
# Install RPM on device
audb package install app.rpm

# Uninstall package
audb package uninstall ru.example.app

# List installed packages
audb package list
audb package list --filter example

# Sign RPM (local, uses Docker)
audb package sign app.rpm

# Validate RPM (local, uses Docker)
audb package validate app.rpm
```

### Shell & File Operations

```bash
# Execute command
audb shell ls -la /home/defaultuser

# Execute as root
audb shell --root cat /etc/passwd

# Push file to device
audb push local.txt /home/defaultuser/remote.txt

# Pull file from device
audb pull /home/defaultuser/file.txt
audb pull /home/defaultuser/file.txt --output local.txt
```

### Input Injection

```bash
# Tap at coordinates
audb tap 360 720

# Long press (500ms)
audb tap 360 720 --duration 500

# Fast tap (direct evdev, requires correct device)
audb tap 360 720 --event auto
audb tap 360 720 --event /dev/input/event4

# Swipe by direction
audb swipe left
audb swipe right
audb swipe up
audb swipe down

# Swipe by coordinates
audb swipe 100 500 600 500

# Fast swipe
audb swipe left --event auto

# Key events
audb key power
audb key home
audb key back
audb key volumeup    # or vol+
audb key volumedown  # or vol-
```

**Note:** Tap and swipe automatically handle screen rotation. Use `--no-rotate` to disable.

### Screenshots

```bash
# Save with auto-generated name
audb screenshot

# Save to specific file
audb screenshot --output screen.png
```

### Application Control

```bash
# Launch app
audb launch ru.example.app

# Stop app
audb stop ru.example.app

# Open URL
audb open https://example.com
audb open file:///home/defaultuser/doc.pdf
```

### Logs

```bash
# Last 100 lines
audb logs

# Last 500 lines
audb logs -n 500

# Filter by priority
audb logs --priority err
audb logs --priority warning

# Filter by unit
audb logs --unit lipstick

# Grep pattern
audb logs --grep "error"

# Since time
audb logs --since "1 hour ago"

# Kernel messages
audb logs --kernel

# Clear logs
audb logs --clear --force
```

### Device Info

```bash
# All info
audb info

# Specific category
audb info device
audb info cpu
audb info memory
audb info battery
audb info storage
audb info features
```

### Server Management

```bash
# Check server status
audb server-status

# Ping server
audb ping

# Start server manually
audb start-server
audb start-server --foreground

# Stop server
audb kill-server

# Force reconnect
audb reconnect
audb reconnect <device>
```

### Global Options

```bash
# Use specific device for this command
audb -d 192.168.2.15 shell uname -a
audb --device my-device info
```

## Configuration

### Device Storage

`~/.config/audb/devices.json`:
```json
{
  "aurora-devices": [
    {
      "name": "My Device",
      "host": "192.168.2.15",
      "port": 22,
      "auth": "/home/user/.ssh/id_rsa",
      "rootPassword": "password",
      "platform": "aurora-arm64",
      "enabled": true
    }
  ]
}
```

### Current Device

`~/.config/audb/current_device` - stores selected device identifier

### Server PID

`~/.config/audb/server.pid` - server process ID

## Architecture

```
┌─────────────┐     Unix Socket     ┌─────────────┐     SSH/SFTP     ┌────────────┐
│  audb CLI   │ ◄─────────────────► │ audb-server │ ◄──────────────► │   Device   │
└─────────────┘                     └─────────────┘                  └────────────┘
```

- **audb** - CLI client, sends commands to server
- **audb-server** - Background daemon, manages SSH connections
- **Connection Pool** - Persistent SSH sessions with auto-reconnect
- **Health Check** - Automatic connection monitoring (60s interval)

## Touchscreen Devices

Known touchscreen event devices:
- **R570**: `/dev/input/event3` (chsc_cap_touch)
- **KVADRA_T**: `/dev/input/event5` (himax-touchscreen)

Use `--event auto` to auto-detect, or specify directly for faster input.

## Troubleshooting

### "No device selected"
```bash
audb device list
audb select 0
```

### "Device disconnected"
```bash
audb reconnect
# or check device status
audb server-status
```

### "Python not found" (tap/swipe)
```bash
# On device:
devel-su pkcon install python3
```

### "Aurora SDK Docker image not found"
Download from Aurora OS Developer Portal and load:
```bash
docker load -i aurora-build-tools-5.2.0.tar.gz
```

### Server issues
```bash
audb kill-server
audb ping  # auto-starts server
```

## Acknowledgments

- Inspired by [aurora-cli](https://gitcode.com/keygenqt_vz/aurora-cli) by Vitaliy Zarubin

## License

MIT License
