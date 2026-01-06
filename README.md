# audb - Aurora OS Device Manager

Device management CLI tool for Aurora OS (similar to Android's adb).

## Features

- Manage multiple Aurora OS devices
- SSH-based device communication
- Remote RPM package installation via D-Bus
- Device selection and state management
- Concurrent connection testing

## Installation

```bash
cd /home/kotdath/omp/personal/rust/aurora-rust-tools/audb
cargo build --release
```

The binary will be available at `target/release/audb`.

## Usage

### Device Management

#### List all devices
```bash
audb device list
```

#### List only active (reachable) devices
```bash
audb device list --active
```

#### Add a new device interactively
```bash
audb device add
```

You'll be prompted for:
- Device name (optional)
- Host IP address
- SSH port (default: 22)
- SSH private key path (default: ~/.ssh/id_rsa)
- Root password
- Platform (aurora-arm or aurora-arm64)

#### Remove a device
```bash
audb device remove <identifier>
```

Where `<identifier>` can be:
- Device index (e.g., `0`, `1`, `2`)
- Device IP address (e.g., `192.168.2.13`)
- Device name (e.g., `my-device`)

### Device Selection

Select the active device for operations:

```bash
audb select <identifier>
```

### Package Installation

Install an RPM package on the selected device:

```bash
audb install path/to/package.rpm
```

This will:
1. Validate the RPM file
2. Connect to the selected device via SSH
3. Upload the package to `/tmp/audb/`
4. Install via D-Bus APM service
5. Clean up temporary files

## Configuration

### Device Storage

Devices are stored in `~/.config/audb/devices.json` with the following format:

```json
{
  "aurora-devices": [
    {
      "name": "My Device",
      "host": "192.168.2.13",
      "port": 22,
      "auth": "/home/user/.ssh/id_rsa",
      "rootPassword": "password",
      "platform": "aurora-arm64",
      "enabled": true
    }
  ]
}
```

### Current Device State

The currently selected device is stored in `~/.config/audb/current_device`.

## Requirements

- Rust 2021 edition
- SSH access to Aurora OS devices
- SSH key-based authentication
- Root password for package installation

## Examples

```bash
# Add a device
audb device add

# List all devices
audb device list

# Select device by index
audb select 0

# Select device by IP
audb select 192.168.2.13

# Install package
audb install my-app-1.0.0-1.armv7hl.rpm

# Check active devices
audb device list --active

# Remove device by name
audb device remove my-device
```

## Architecture

- **SSH/SFTP**: Communication via russh library
- **Authentication**: SSH key-based (defaultuser account)
- **Installation**: D-Bus APM service (ru.omp.APM.Install method)
- **Configuration**: JSON-based device storage
- **Async Runtime**: Tokio for concurrent operations

## Notes

- Default SSH user is `defaultuser` (Aurora OS standard)
- SSH host key checking is disabled (development mode)
- Connection timeout: 5 seconds
- Session timeout: 30 seconds
