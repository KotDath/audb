# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The framebuffer panel size now comes from the framebuffer var
  (`FBIOGET_VSCREENINFO`) when the service unit allows `/dev/fb0`, falling back
  to a non-empty `mode` and then to the first `modes` line. The var is rejected
  when `rotate` is neither 0 nor 180, when the size is outside the supported
  range, or when the kernel publishes a `modes` list that does not contain it;
  a non-empty but unusable list is not treated as no list. With no list at all
  the driver value is taken as the visible size. These are conditional checks,
  not a guarantee that an accepted size is native geometry. The unit adds a
  read-only `DeviceAllow=/dev/fb0 r` for this. `status` reports `panel.source`
  and, when the var was rejected or unusable and a text source still resolves a
  size, a `panel.note`; if no source resolves one, status reports the panel
  error instead.
- The agent service unit grants the root service read-open access to `/dev/fb0`
  through `DeviceAllow=/dev/fb0 r`. That permission applies for the lifetime of
  the unit on every deployment, not only while the startup geometry getter runs,
  and it is not an ioctl-only or geometry-only boundary: it permits reading the
  framebuffer and whatever a read-only descriptor on that driver permits. It is
  an accepted developer-device trade-off, not production hardening.
- Panel geometry stays a best-effort kernel reading. When a framebuffer has no
  active `mode`, the first line of `modes` is used; the kernel prepends only
  unmatched modes, so a multi-mode list can keep a non-active mode first. No
  multi-mode device has been demonstrated, and treating such lists as ambiguous
  would disable panels that work today.
- A session geometry that disagrees with the startup node is reported on
  `status` as `PANEL_GEOMETRY_MISMATCH` with both native sizes, `tap` and `swipe`
  are no longer advertised as ready, and doctor shows a touch warning without
  asking for a reinstall. Gestures stay fail-closed with `INPUT_FAILED`, while
  screenshots, keyboard and permissions are unaffected even when the two
  geometries disagree.

### Fixed

- Physical touch on framebuffer panels that have no DRM connector. The agent
  reads the fbdev mode file and falls back to modes. Keyboard, permissions and
  screenshots stay available when no panel mode exists. doctor stays healthy
  and reports a touch warning instead of asking for another device setup after
  the agent is already installed.
- DRM connector symlinks are followed, so an existing KMS panel is not replaced
  by a framebuffer mode.

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
