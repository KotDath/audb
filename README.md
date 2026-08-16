# audb — Aurora Emulator Debug Bridge

`audb` is an emulator-only Aurora OS automation CLI. It provides the audb2 command set in one Rust binary and is designed for direct `execFile`/argv integration with automation clients such as claude-in-mobile.

Physical Aurora devices are intentionally not supported on `main` yet. The previous implementation is preserved in `backup/physical-devices-v0.1.0`.

## Requirements

- Aurora SDK (default: `/home/kotdath/AuroraOS`, override with `AURORA_SDK_ROOT`)
- Aurora SDK emulator `AuroraOS-5.2.0.180`
- Rust toolchain for building
- Docker and a local Aurora Build Tools image only for `package sign` and `package validate`

No helper application is installed on Aurora OS. Input uses QEMU QMP, screenshots use Lipstick's D-Bus API with QMP fallback, and guest operations use the SDK SSH key.

## Build and setup

```bash
cargo build --release
target/release/audb install
target/release/audb emulator start
target/release/audb status
```

`audb install` is reversible and does not start or restart the emulator. It:

- wraps the SDK's QEMU binary matching the host architecture (`qemu-system-x86_64` on Linux, `qemu-system-aarch64` on Apple Silicon macOS) and preserves the original as `.real`;
- adds a QMP Unix socket and virtual multitouch/keyboard devices;
- enables SDL mouse interaction and a visible host cursor;
- migrates an existing audb2 wrapper safely.

Use `audb uninstall` to restore the original QEMU binary and pointing-device files.

## Automation contract

Pass arguments without a shell and add global `--json` for one stable response document:

```bash
audb --json device current
audb --json tap 180 400
audb --json screenshot --output /tmp/screen.png
audb --json app pid ru.example.App
```

Success:

```json
{"ok":true,"deviceId":"emulator","data":{}}
```

Failure:

```json
{"ok":false,"deviceId":"emulator","error":{"code":"QMP_ERROR","message":"..."}}
```

The public process starts a private daemon mode automatically. The daemon keeps SSH and QMP sessions alive, but there is no separate server binary or public server-management API.

## Commands

```text
tap, swipe, text, key, screenshot, status
install, uninstall, setup-status
emulator start|stop|status
device list|current, select
shell, push, pull, open, info, logs
launch, stop, app ...
display status|on|off|dim|lock|wake
perf snapshot|monitor|visual-fps
crash list|watch|clear
sandbox paths|list|pull|sqlite
network status|interfaces|traffic|proxy|offline
location set|track
sensor list|enable|disable|set-vector|set-scalar
clipboard status|get|set|clear
package list|install|uninstall|sign|validate
```

Run `audb <command> --help` for arguments. `clipboard status` reports the known emulator limitation; mutating clipboard commands return `CAPABILITY_UNAVAILABLE`.

Useful examples:

```bash
audb tap 180 400
audb swipe up
audb swipe fast-left
audb swipe edge-up
audb text "Hello Aurora"
audb screenshot --output screen.png

audb app launch ru.example.App
audb app wait-running ru.example.App --timeout 15
audb display lock
audb sandbox sqlite ru.example.App data database.sqlite "select * from items limit 10"
audb network proxy set 127.0.0.1 8080
audb location set 55.751244 37.618423
```

## Safety notes

- `-d/--device` accepts only `emulator`; device registry mutations return `UNSUPPORTED_IN_EMULATOR_ONLY`.
- Sandbox paths are canonicalized in the guest and cannot escape an application's private roots.
- SQLite accepts only read-only query prefixes and limits output to 1000 rows.
- `app clear-data` requires either `--dry-run` or explicit `--confirm`.
- `logs --clear` requires `--force`.
