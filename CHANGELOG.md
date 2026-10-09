# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-10-09

### Added

- Registered physical devices and emulators, explicit `--device` targeting,
  atomic device registry updates and migration from emulator configuration.
- Aurora system agent with physical tap/swipe, PNG screenshots, Unicode text
  through Maliit, named keyboard keys and Sailjail permission management.
- `setup-device` and `package install-system` for explicitly installing
  developer system RPMs, with preflight checks and read-back verification.
- `doctor` and `capabilities` with structured readiness, backend, geometry,
  failure reasons and suggested fixes for automation clients.

### Changed

- Application RPM installation streams through SFTP, checks metadata and
  architecture, waits for matching APM registration and skips an already
  installed version. Large RPMs no longer occupy a JSON request frame.
- Emulator gestures use current QMP display dimensions with coordinate bounds
  checks and proportional direction aliases.
- CLI JSON includes the resolved device ID, stable error codes and operation
  deadlines. Unknown outcomes are reported without replaying modifying actions.
- CLI/daemon protocol is now 10; the Aurora agent protocol is 3. Old daemons use
  separate sockets. Physical input, screenshots and permissions need the agent;
  installing the host CLI does not automatically install device packages.

### Fixed

- QMP readiness probes release cached connections before using a disposable
  client, supporting QEMU endpoints that accept only one client.
- Screenshot staging uses unique directories, and failed local captures leave
  existing output files intact.

### Verification and scope

- 91 workspace tests and Clippy pass. The common automation scenario was checked
  on Aurora 5.2.0.259/aarch64 hardware and Aurora 5.2.1.200/x86_64 emulator.
- Agent RPMs are developer system packages, not production-validated Regular
  applications. UI tree and clipboard are unsupported; other physical
  orientations and real emulator resolution changes remain unverified.

## [0.2.2] - 2026-09-07

### Changed

- Screenshot capture now prefers QMP, falls back to capturing the host QEMU
  window, and uses Lipstick as the final fallback with combined diagnostics.
- The QEMU wrapper replaces GL display devices with software-rendered variants
  so QMP `screendump` can access the emulator surface reliably.

## [0.2.1] - 2026-08-16

### Fixed

- `audb install` now works on macOS (Apple Silicon): the QEMU binary name is
  derived from the host architecture (`qemu-system-aarch64` on ARM, kept
  `qemu-system-x86_64` on x86-64) instead of being hardcoded.
- The wrapper installer recognizes Mach-O binaries (thin and fat) as native
  executables, not only ELF, so it no longer refuses to wrap the macOS QEMU
  binary.

## [0.2.0] - 2026-07-18

### Changed

- Emulator-only runtime implemented on top of QEMU: the SDK's QEMU binary is
  wrapped with QMP socket and virtual input device injection, input uses QMP,
  screenshots use Lipstick's D-Bus API with QMP fallback, and guest operations
  use the SDK SSH key.
- Full audb2 emulator command surface ported into a single Rust binary.

## [0.1.0]

### Added

- Initial baseline release.
