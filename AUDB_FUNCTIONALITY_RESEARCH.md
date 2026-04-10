# AUDB Functionality Research

## Scope

Этот документ фиксирует:

- полный текущий CLI surface `audb`;
- фактическую реализацию каждой команды по коду, а не по README;
- целевой canonical path, по которому команды нужно довести;
- разрывы между уже существующим кодом и целевым поведением.

Исследование основано на:

- `audb-client/src/main.rs`
- `audb-server/src/socket_server.rs`
- `audb-server/src/pool.rs`
- `audb-core/src/features/**`
- `app/AudbBridge/src/**`

## Canonical Architecture

Сейчас в проекте одновременно живут две модели:

1. старая модель из `audb-core`:
   - команды напрямую открывают SSH;
   - input и screenshot идут через Python/root или старый D-Bus lipstick path;
2. новая модель client/server:
   - `audb` является thin client;
   - `audb-server` держит persistent SSH connections;
   - input идёт через `AudbBridge` D-Bus helper на устройстве.

Canonical direction для проекта:

- локальные команды остаются локальными:
  - `device add`
  - `device list`
  - `device remove`
  - `select`
  - `package sign`
  - `package validate`
- все device-facing команды идут через `audb-server`;
- transport между client и server: `audb-protocol`;
- device execution path: `audb-server -> ConnectionPool -> persistent SSH -> remote command`;
- input и screenshot должны идти через `ru.kotdath.AudbBridge`, а не через legacy Python scripts в `audb-core`.

## Full Functionality Inventory

Текущий CLI surface:

```text
audb device list
audb device add
audb device remove
audb package install
audb package uninstall
audb package list
audb package sign
audb package validate
audb select
audb ping
audb start-server
audb kill-server
audb server-status
audb shell
audb push
audb pull
audb info
audb tap
audb swipe
audb key
audb screenshot
audb launch
audb stop
audb logs
audb reconnect
audb open
```

Supporting functionality outside top-level commands:

- selected device persistence in `~/.config/audb/current_device`;
- device registry in `~/.config/audb/devices.json`;
- server PID/log management in `~/.config/audb/server.pid` and `server.log`;
- auto-start of `audb-server` when client sends a device command;
- SSH command execution and SFTP upload/download;
- `AudbBridge` device app with D-Bus methods:
  - `Tap`
  - `Swipe`
  - `SwipeDirection`
  - `Key`
  - `Screenshot`

## Command Matrix

### Device Management

#### `audb device list`

- Current state: implemented locally.
- Current implementation:
  - reads enabled devices from `DeviceStore`;
  - with `--active` concurrently probes SSH reachability;
  - without `--active` it can also merge live server status from the Unix socket.
- Canonical implementation:
  - keep local;
  - keep optional server-status merge;
  - make output the single source of truth for selection identifiers: index, name, host.
- Gaps:
  - output formatting can drift depending on whether server is running;
  - “active” means SSH reachable, not necessarily `AudbBridge` healthy.

#### `audb device add`

- Current state: implemented locally and interactive.
- Current implementation:
  - asks for device metadata via `dialoguer`;
  - tests SSH connection;
  - stores host, port, key path, root password, platform in config.
- Canonical implementation:
  - keep local;
  - add optional non-interactive flags later for CI;
  - validate not only SSH, but also optional `AudbBridge` availability.
- Gaps:
  - no explicit capability probe for `AudbBridge`;
  - root password is still collected even if some commands should not need root anymore.

#### `audb device remove`

- Current state: implemented locally.
- Current implementation:
  - resolves identifier by index/name/host;
  - removes from config store.
- Canonical implementation:
  - keep local;
  - if removed device is selected, clear current selection;
  - if server is running, optionally evict from pool.
- Gaps:
  - current server hot-unregister path does not exist.

#### `audb select`

- Current state: implemented locally.
- Current implementation:
  - resolves identifier and writes selected host to `~/.config/audb/current_device`.
- Canonical implementation:
  - keep local;
  - store a stable device id if registry schema evolves;
  - continue resolving to host for protocol until ids are introduced.
- Gaps:
  - current selection is a plain string host/name resolution output, not a stable UUID.

### Server Management

#### `audb ping`

- Current state: implemented through protocol.
- Current implementation:
  - sends `Command::Ping`;
  - server returns `pong`.
- Canonical implementation:
  - keep as health check for socket/protocol only;
  - add optional `--device` health probe later if needed.
- Gaps:
  - does not validate SSH or `AudbBridge`.

#### `audb start-server`

- Current state: implemented locally.
- Current implementation:
  - finds `audb-server` binary in PATH, next to executable, or cargo targets;
  - foreground or daemon mode.
- Canonical implementation:
  - keep local;
  - keep auto-start behavior in client for device commands.
- Gaps:
  - no structured readiness handshake beyond socket polling.

#### `audb kill-server`

- Current state: implemented locally via PID file, not via protocol.
- Current implementation:
  - reads `~/.config/audb/server.pid`;
  - sends `SIGTERM`;
  - cleans stale PID/socket files.
- Canonical implementation:
  - keep local PID-based kill as primary behavior;
  - remove or de-emphasize protocol `KillServer`, because server-side command currently only returns success text.
- Gaps:
  - protocol `Command::KillServer` is misleading and not wired to shutdown.

#### `audb server-status`

- Current state: implemented through protocol.
- Current implementation:
  - server reports PID, socket path, per-device connection state and command counters from `ConnectionPool`.
- Canonical implementation:
  - keep;
  - add real uptime instead of current placeholder `0`.
- Gaps:
  - uptime is TODO;
  - no per-device feature/capability health.

### Shell and Transfer

#### `audb shell`

- Current state: implemented through protocol.
- Current implementation:
  - thin client builds `Command::Shell`;
  - server queues execution on persistent SSH session;
  - optional root path uses `devel-su`.
- Canonical implementation:
  - keep exactly in server path;
  - preserve root mode for administrative commands;
  - add better stderr/stdout separation later if needed.
- Gaps:
  - output is flattened into text lines;
  - remote command execution is shell-string based.

#### `audb push`

- Current state: implemented through protocol.
- Current implementation:
  - client reads local file to memory;
  - protocol sends bytes;
  - server writes temp file locally and uploads via SFTP.
- Canonical implementation:
  - keep protocol + pool + SFTP path;
  - later switch to chunked streaming if large files become a problem.
- Gaps:
  - full file currently buffered in client and server.

#### `audb pull`

- Current state: implemented through protocol.
- Current implementation:
  - server downloads remote file to temp path via SFTP;
  - server reads bytes and returns binary payload;
  - client writes output file.
- Canonical implementation:
  - keep for now;
  - later support chunked/streamed transfer.
- Gaps:
  - full file buffered in memory;
  - no resume/progress support.

### Package Management

#### `audb package install`

- Current state: implemented through protocol.
- Current implementation:
  - client reads RPM;
  - server uploads to `/home/defaultuser/Downloads`;
  - server calls `ru.omp.APM.Install` over system D-Bus;
  - cleanup temp file.
- Canonical implementation:
  - keep server-side install via APM;
  - add parsing of D-Bus result into a stable user-facing success/error format.
- Gaps:
  - full RPM transferred in-memory;
  - minimal validation before install.

#### `audb package uninstall`

- Current state: implemented through protocol.
- Current implementation:
  - server calls `ru.omp.APM.Remove`.
- Canonical implementation:
  - keep APM D-Bus path;
  - normalize package name validation;
  - surface “not installed” cleanly.
- Gaps:
  - no preflight check.

#### `audb package list`

- Current state: implemented through protocol.
- Current implementation:
  - server calls `ru.omp.APM.GetPackageList`;
  - parses `'general.id': '...'` from textual gdbus output.
- Canonical implementation:
  - keep server-side package query;
  - replace fragile string scraping with structured D-Bus parsing if possible.
- Gaps:
  - current parser is brittle and tied to `gdbus` textual formatting.

#### `audb package sign`

- Current state: implemented locally.
- Current implementation:
  - downloads default key/cert into `~/.cache/audb` unless custom paths are provided;
  - searches for Aurora SDK Docker image;
  - copies key/cert into project dir;
  - runs `rpmsign-external sign` in Docker.
- Canonical implementation:
  - keep local;
  - keep Docker-based workflow;
  - add explicit image override env/flag later.
- Gaps:
  - depends on `curl` and `docker`;
  - Docker image discovery is heuristic.

#### `audb package validate`

- Current state: implemented locally.
- Current implementation:
  - locates Docker image;
  - runs `rpm-validator -p regular` in container;
  - marks failure if output contains `(ERROR)`.
- Canonical implementation:
  - keep local;
  - keep Docker-based validation;
  - later emit structured diagnostics.
- Gaps:
  - output parsing is heuristic;
  - depends on image naming heuristics.

### Device Information

#### `audb info`

- Current state: implemented through protocol.
- Current implementation:
  - server gathers data from multiple sources:
    - `ru.omp.deviceinfo.Features`
    - `com.nokia.mce`
    - `/proc/meminfo`
    - `stat -f /home`
  - client formats either full or category-specific output.
- Canonical implementation:
  - keep server-side aggregation;
  - keep protocol response as structured `DeviceInfo`;
  - later split per-category fetching to reduce latency if needed.
- Gaps:
  - parsing uses ad hoc extraction from textual gdbus output;
  - category is ignored server-side and only affects client formatting.

### Input Injection

#### `audb tap`

- Current state: implemented through protocol and `AudbBridge`.
- Current implementation:
  - server builds session-bus `gdbus call` to `ru.kotdath.AudbBridge.Tap`;
  - options map supports `eventDevice` and `durationMs`.
- Canonical implementation:
  - keep `AudbBridge` as the only supported path;
  - remove or deprecate legacy `audb-core` Python-script path from `audb-core/src/features/input/tap.rs`;
  - optionally expose more options already present in `AudbBridge`, not just current CLI fields.
- Gaps:
  - server assumes session bus at `/run/user/$(id -u)/dbus/user_bus_socket`;
  - no explicit bridge-availability probe before sending command.

#### `audb swipe`

- Current state: implemented through protocol and `AudbBridge`.
- Current implementation:
  - server calls either `ru.kotdath.AudbBridge.Swipe` or `SwipeDirection`;
  - direction mapping:
    - `left -> rl`
    - `right -> lr`
    - `up -> du`
    - `down -> ud`
- Canonical implementation:
  - keep `AudbBridge`;
  - extend CLI later with `--steps`, `--step-delay-ms`, since bridge already supports them.
- Gaps:
  - CLI does not expose all bridge options;
  - legacy Python swipe code in `audb-core` is obsolete relative to server path.

#### `audb key`

- Current state: implemented through protocol and `AudbBridge`.
- Current implementation:
  - server calls `ru.kotdath.AudbBridge.Key`.
- Canonical implementation:
  - keep `AudbBridge`;
  - document exact supported key names from helper implementation.
- Gaps:
  - no validation table on client side;
  - user gets runtime failure if key alias is unsupported.

### Screenshots

#### `audb screenshot`

- Current state: partially implemented, but architecture is inconsistent.
- Current implementation:
  - client requests binary screenshot from server;
  - server still uses old lipstick D-Bus path:
    - `org.nemomobile.lipstick.saveScreenshot`
  - server reads the remote file via `base64`.
- Canonical implementation:
  - migrate screenshot to `AudbBridge.Screenshot`;
  - bridge should own fallback chain:
    - StreamCamera
    - QML screenshot grabber
    - screen-grab backend if suitable
    - legacy lipstick fallback only as last resort if still needed
  - server should only:
    1. call `AudbBridge.Screenshot(outputPath)`;
    2. pull the generated file;
    3. delete temp file.
- Gaps:
  - current server screenshot path ignores the richer screenshot stack already implemented in `app/AudbBridge`;
  - current implementation still requires root and base64 roundtrip.

### Application Control

#### `audb launch`

- Current state: implemented through protocol.
- Current implementation:
  - server calls `ru.omp.RuntimeManager.Control1.Start`.
- Canonical implementation:
  - keep RuntimeManager system D-Bus path;
  - keep input validation for D-Bus-like app names.
- Gaps:
  - currently just relays raw gdbus output.

#### `audb stop`

- Current state: implemented through protocol.
- Current implementation:
  - server calls `ru.omp.RuntimeManager.Control1.Terminate`.
- Canonical implementation:
  - keep RuntimeManager path.
- Gaps:
  - same as launch: raw command output only.

#### `audb open`

- Current state: implemented through protocol.
- Current implementation:
  - server calls `org.sailfishos.fileservice.openUrl` on session bus.
- Canonical implementation:
  - keep session-bus openUrl path;
  - later add URL/path validation by scheme.
- Gaps:
  - minimal validation;
  - depends on session bus and fileservice availability.

### Logs

#### `audb logs`

- Current state: implemented through protocol.
- Current implementation:
  - server builds `journalctl` command with filters:
    - `-n`
    - `-p`
    - `-u`
    - `--since`
    - `grep`
    - `-k`
    - clear mode via rotate/vacuum
  - root execution through `devel-su`.
- Canonical implementation:
  - keep server-side log collection via `journalctl`;
  - keep validation rules:
    - `lines > 0`
    - no `--kernel` with `--unit`
    - `--clear` requires `--force`
  - later consider streaming/follow mode separately.
- Gaps:
  - `grep` is a shell pipe, not a structured filter;
  - output is text only.

### Reconnect

#### `audb reconnect`

- Current state: declared in client and protocol, but not implemented in server.
- Current implementation:
  - server returns `Reconnect command not yet implemented in Phase 1`.
- Canonical implementation:
  - implement in `ConnectionPool` as explicit session invalidation:
    - target one device or all devices;
    - drop persistent SSH handle;
    - reset state to `Disconnected`;
    - next command re-establishes SSH;
  - optionally add an eager reconnect mode for immediate validation.
- Gaps:
  - entirely missing.

## Supporting Subsystems

### Device Registry and Selection

Current behavior:

- devices are loaded from local config;
- server loads enabled devices at startup only;
- selected device is read by client and converted into protocol `device` field.

Canonical direction:

- keep client-side resolution for now;
- later add server-side hot reload or explicit add/remove/reload commands if device list changes while daemon is alive.

### ConnectionPool

Current behavior:

- one queue per device;
- commands execute serially per device;
- pool keeps persistent SSH connection;
- health check every 60 seconds with `echo 1`;
- reconnect is lazy on next command after failure.

Canonical direction:

- keep this as the execution core;
- add explicit reconnect/drop-session APIs;
- later add upload/download streaming if needed.

### AudbBridge App

Current behavior:

- D-Bus service `ru.kotdath.AudbBridge`;
- supports:
  - tap
  - swipe
  - swipeDirection
  - key
  - screenshot
- screenshot path already has a richer strategy than server:
  - StreamCamera
  - QML grabber
  - D-Bus fallback

Canonical direction:

- make it the single authority for input and screenshot;
- stop duplicating legacy input/screenshot logic in `audb-core`.

## Mismatch Between README and Code

Important mismatches discovered:

- README says input no longer needs Python on target.
  - This is true for the server path.
  - It is false for legacy `audb-core/src/features/input/*.rs`, which still upload Python scripts.
- README presents screenshots as a first-class feature.
  - CLI and server have it.
  - But server is still using old lipstick screenshot flow instead of `AudbBridge.Screenshot`.
- README suggests bridge-centric architecture.
  - The actual repository still contains legacy direct SSH feature modules that are no longer canonical for input/screenshot.
- `reconnect` is documented by CLI surface, but not implemented server-side.
- protocol `KillServer` exists, but real shutdown is done by local PID kill path, not by server command processing.

## What I Would Implement Next

Priority order:

1. make `screenshot` use `AudbBridge.Screenshot` end-to-end;
2. implement real `reconnect` in `ConnectionPool`;
3. remove or deprecate legacy `audb-core` input/screenshot execution paths;
4. expose richer swipe options already supported by `AudbBridge`;
5. harden `package list` and `info` parsing away from brittle textual `gdbus` scraping;
6. add capability probing:
   - SSH reachable
   - session bus reachable
   - `AudbBridge` registered
7. add server hot reload or explicit refresh for device registry changes.

## Command-by-Command Implementation Plan

### Keep as-is with cleanup

- `device list`
- `device add`
- `device remove`
- `select`
- `ping`
- `start-server`
- `kill-server`
- `server-status`
- `shell`
- `push`
- `pull`
- `package install`
- `package uninstall`
- `package sign`
- `package validate`
- `launch`
- `stop`
- `open`
- `logs`

### Keep, but harden

- `package list`
- `info`
- `tap`
- `swipe`
- `key`

### Rework

- `screenshot`
  - move to `AudbBridge.Screenshot`
  - eliminate root/base64 screenshot path from server

### Implement from scratch

- `reconnect`
  - add explicit pool/session reset command

## Final Recommended Rule Set

To avoid future drift, the project should follow these rules:

- `audb-client` owns CLI UX, local config, and local-only workflows.
- `audb-server` owns all device-side operations over persistent SSH.
- `audb-core` should not keep alternate legacy execution paths for commands already owned by server.
- `AudbBridge` is the only supported backend for input and screenshot.
- README should document only the canonical path, not historical implementations.
