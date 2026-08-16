# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
