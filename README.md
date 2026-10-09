# audb — Aurora Debug Bridge

`audb` is an Aurora OS automation CLI for registered emulators and physical devices. It provides the audb2 command set in one Rust binary and is designed for direct `execFile`/argv integration with automation clients such as claude-in-mobile.

Physical devices support SSH shell, SFTP push/pull, status, agent-backed tap/swipe/screenshots, Unicode text, named keys, permission management and common Aurora guest commands where the device permits them. Install the system agent with `setup-device` before using physical input, screenshots or permissions. The previous implementation is preserved in `backup/physical-devices-v0.1.0`.

Install the host CLI release with `cargo install audb-client --version 0.3.0 --locked`. The executable is named `audb`. Device agent RPMs are distributed separately in the [v0.3.0 release](https://github.com/KotDath/audb/releases/tag/v0.3.0); choose the device architecture and pass the package explicitly to `setup-device --rpm`. Cargo installation does not bundle or install a device RPM.

## Requirements

- OpenSSH `ssh` and `sftp` for physical devices; existing SSH profiles and agent identities are supported
- Aurora SDK for emulator operations (default: `/home/kotdath/AuroraOS`, override with `AURORA_SDK_ROOT`)
- Aurora SDK emulator `AuroraOS-5.2.0.180`, or a registered emulator with custom SDK/QMP settings
- Rust 1.87 or newer for building the agent (verified with stable 1.96)
- Docker and a local Aurora Build Tools image only for `package sign` and `package validate`

Registering a device does not install a helper or start a VM. Emulator input uses QEMU QMP; screenshots try QMP, host window capture and Lipstick. Emulator guest operations retain the SDK SSH transport.

## Register and target a device

```bash
# Establish SSH access and verify the host key first, if it is not already known.
ssh defaultuser@192.168.2.44

audb device add defaultuser@192.168.2.44 --id phone
audb --json device list
audb --device phone --json status
audb --device phone shell uname -a
audb --device phone push ./file.bin /home/defaultuser/file.bin
audb --device phone pull /home/defaultuser/file.bin ./download.bin

# Optional explicit key, port, display name and root SSH account.
audb device update phone --name "Aurora tablet" --key /path/to/key --port 22
# Return to SSH profile/agent identities:
audb device update phone --ssh-config

# Default for manual commands; explicit --device never changes it.
audb select phone
audb --json device current
audb device remove phone
```

The full registration form accepts `--host`, `--user`, `--port`, `--key`, `--kind physical|emulator` and `--root-user`. An SSH profile name can replace `user@host`; its configured user and port are resolved locally at registration. Without an explicit identity, physical devices use OpenSSH's configured identities and SSH agent. Host keys must already be trusted in `known_hosts`; audb does not accept new or changed keys automatically.

Emulators additionally accept `--qmp`, `--sdk-root` and `--emulator-name`. Ordinary privileged guest commands currently require root SSH. System package installation also supports one-shot devel-su authentication; audb does not store a devel-su password.

### One-time root SSH setup (source build; not included in released 0.3.0)

```sh
audb --device phone setup-root
audb --device phone setup-root --check-only
audb --device phone shell --root 'id -u'
```

`setup-root` first tests root SSH. If it already works, no password is needed.
Otherwise it asks for a hidden `devel-su` password once, creates an Ed25519
identity for this device under `ssh-identities/` beside the registry, and adds
the public key to the existing SSH user and UID-0 account. It verifies both
connections before atomically saving the key path and root account. Repeating
setup returns `changed: false`. Setup preserves the selected default and works
with each device's registered host/profile, port and account; emulator SDK root
access is reused. Noninteractive setup accepts `--root-password-stdin`; the
credential never appears in argv, JSON output or the registry.

`--check-only` changes nothing. SSH must permit root public-key authentication
and use the account's `.ssh/authorized_keys`. Setup does not edit `sshd` policy.
If verification fails, newly added key lines are removed where possible, and
the registry is unchanged. Inspect `data.keyCleanup` on errors or existing keys
after an unknown outcome. Existing keys and the generated local identity remain
available; do not blindly retry interrupted setup.

The registry is `${XDG_CONFIG_HOME:-~/.config}/audb/devices-v1.json`. On first use, `emulator.json` is migrated with ID `emulator`; the original and a `emulator.json.pre-registry.bak` backup remain available. When no old emulator configuration exists, the default emulator is registered for compatibility. An older `devices.json` uses a different schema and is left untouched. Writes are atomic and registry mutations are locked.

Without `--device`, audb uses the saved default or the only registered device. Ambiguous or empty registries return `DEVICE_REQUIRED`; an unknown ID returns `DEVICE_NOT_FOUND`. `device list` reads registration metadata without network probes and reports state `unknown`; use targeted `status` to probe connectivity.

## Build and setup

```bash
cargo build --release -p audb-client
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

## System RPM installation and device bootstrap

```bash
# General installer for a supplied system RPM:
audb --device phone package install-system ./system-package.rpm --check-only
audb --device phone package install-system ./system-package.rpm

# Bootstrap accepts only an RPM whose package name is audb-agent:
audb --device phone setup-device --rpm ./audb-agent.rpm

# Updating and replacing the same version are separate explicit choices:
audb --device phone package install-system ./system-package.rpm --upgrade
audb --device phone package install-system ./system-package.rpm --upgrade --reinstall
```

The installer uploads to a unique directory, reads RPM metadata, checks the userspace architecture using `rpm --eval '%{_arch}'`, and accepts matching or `noarch` packages. It first runs `rpm -ivh --test --undefine=__transaction_validation`, then `rpm -ivh --undefine=__transaction_validation`, both as root. `--upgrade` selects `-Uvh`; `--reinstall` adds `--replacepkgs`. `--check-only` stops after the test transaction and returns `installed: false`. A completed install is confirmed against the RPM database.

This is the explicit developer-device route used for the system service. The override applies to these RPM invocations; it does not change persistent validator configuration. Existing `package install` continues to use APM. Package signing remains a separate operation via `package sign`.

With configured root SSH, the installer uses it. Otherwise, a manual terminal invocation requests a hidden password for devel-su. Agent integrations supply one password line on stdin and add `--root-password-stdin`; the credential is passed over the private CLI/daemon socket and SSH stdin, without saving it in configuration or putting it in remote command arguments. Missing noninteractive credentials return `ROOT_ACCESS_REQUIRED`. An explicitly supplied credential chooses devel-su even when a root SSH account is configured.

Errors include `phase` (`check`, `install`, or `verify`) where available. A lost response or deadline returns `OUTCOME_UNKNOWN` and never retries the transaction. Uploaded files are cleaned on success and failure; cancellation triggers best-effort cleanup. `--json` preserves one response document.

`setup-device` delegates to the general installer and checks RPM Name `audb-agent`; it does not install arbitrary packages through that wrapper. Without `--rpm`, it looks for `packages/audb-agent.rpm` beside the executable. Build the package with `python3 scripts/build-agent-rpm.py`, sign the exact resulting RPM, then pass its path to `setup-device`. RPM scripts enable and start the service; setup also queries the agent in the graphical session and returns its geometry and capabilities. If readiness fails after installation, the error includes `phase: agent_verify` and `rpmTransactionCompleted: true`. A missing default package reports `NOT_FOUND`.

On `phone`, the test transaction for the existing signed `aurora-strongswan-0.1.0-3.aarch64.rpm` succeeded with `--check-only --upgrade --reinstall`. An obsolete `aurora-strongswan-runtime` RPM was correctly rejected during `check`. Neither probe installed a package or restarted a service.

## Physical touchscreen

```bash
# From this checkout (the globally installed audb may still be an older version):
./target/debug/audb --device phone --json status
./target/debug/audb --device phone --json tap 940 323
./target/debug/audb --device phone --json tap 940 323 --duration 1000
./target/debug/audb --device phone --json swipe 300 950 300 350 --duration 800 --hold 80
./target/debug/audb --device phone --json swipe up
./target/debug/audb --device phone --json swipe edge-up
```

Coordinates are pixels in the current oriented screen, with the origin at its top-left. Geometry is read from Qt Wayland on every command, and checked against the connected DRM panel by the service. There are no hardcoded tablet dimensions, UID or input group GID. Direction aliases use that geometry; supported names match the emulator (`up/down/left/right`, `edge-*`, `fast-*`, `long-*`). `--duration`, `--hold`, and `--steps` remain available. Tap duration is 1–3000 ms; swipe duration is 40–3000 ms, hold 0–1000 ms, steps 1–240. Invalid coordinates/options return `INVALID_ARGUMENT` before sending events. A successful command confirms event delivery, not that a particular application accepts that gesture. On the tested tablet, home uses `edge-up`; `edge-right` at the top-level Help page has no effect.

`audb-agent` and `audb-agentctl` are Rust binaries. Small C++ Qt components query geometry and bridge the active Maliit text context. The root systemd service owns one persistent uinput touchscreen; it does not expose shell execution or arbitrary file access. Its Unix socket is root:input 0660, with SO_PEERCRED and group membership checks. Normal input commands use user SSH and JSON on stdin; they require no root password after setup. Contacts are released on completion, failure, local agent-client socket disconnect or service termination. An SSH/CLI deadline does not guarantee immediate remote cancellation: an already submitted gesture can finish within the service limits (up to 4 seconds plus a 250 ms settle). A lost response returns `OUTCOME_UNKNOWN` without replaying the action.

The aarch64 developer package `audb-agent-0.3.0-3.aarch64.rpm` was installed and launched on `phone` (KVADRA_T, Aurora 5.2.0.259). Visually confirmed opening Help, selecting a category, swiping back, swiping from the bottom edge to home, and scrolling Settings in both directions. Evidence is in `target/touch-evidence/`; `setup-final.json` records installation/readiness. Tests cover rotation/bounds, framing/deadlines, releasing a contact on disconnect, CLI routing/stdin and setup. Other physical devices and portrait/inverted orientations have not yet been verified on hardware; emulator touch input continues through QMP; text and named keys prefer the shared agent when available.

The build script uses Aurora SDK 5.2.1.200 sb2 and rpmbuild, following the local StrongSwan build route and the official [Scratchbox2 documentation](https://developer.auroraos.ru/doc/5.2.0/sdk/tools/scratchbox2). Override the SDK container/target, Rust target and RPM architecture with script options when building for another target; the matching Rust target must already be installed. This build is signed with the Regular development certificate. `rpm-validator -p regular` (SDK tool version reported as unknown) exits 1: system services, RPM scriptlets and system executable locations do not fit the regular application profile; it also reports missing desktop/icons/name conventions and an empty-rpath warning. Installation uses the explicit developer system-RPM route with the transaction validation override. No trusted root certificate was available for separate signature trust verification. This is not a validated production distribution package.

## Physical screenshots

```bash
# JSON metadata and an atomically saved PNG:
./target/debug/audb --device phone --json screenshot --output ./screen.png

# Only PNG bytes on stdout (no --json):
./target/debug/audb --device phone screenshot > ./screen.png
```

The JSON result includes `deviceId`, absolute `output`, `format: "png"`, `bytes`, `width` and `height`. JSON mode requires `--output`; without JSON and without an output path, stdout contains the image and errors go to stderr. A failed capture leaves an existing output file intact. Automation can use either argv plus JSON metadata, or capture binary stdout directly.

Physical capture requires a running graphical session and an agent advertising `screenshot: true` (upgrade older packages through `setup-device`). After setup it uses ordinary user SSH without a root password. The service requests a Lipstick capture over the caller's session D-Bus, using the authenticated socket peer UID. The user client creates a unique directory under its passwd home, waits for a complete PNG with valid chunk CRCs (up to 16 MiB), cleans the staging directory, then sends a JSON header and raw PNG through SSH. The host strips the internal header. There is no fixed one-second delay or runtime dependency on Flutter/Python. The root service does not read image files. Its systemd unit hides home directories with `ProtectHome=tmpfs` and exposes `/run/user` read-only for session bus access.

Lipstick shows a system screenshot-saved notification on this device; a subsequent capture can include that notification while it is visible.

Normal completion, capture errors and bounded waiting clean the user staging directory; handled SIGINT/SIGTERM cancellation also exits through cleanup. Abrupt SIGKILL or device failure can leave a directory behind. An SSH/CLI deadline does not guarantee immediate remote cancellation.

Screenshot support was introduced in release 5; `phone` now runs signed `audb-agent-0.3.0-10.aarch64.rpm`. Verified actual PNG decoding at 2000×1200, raw stdout, tap → changed screenshot, edge-up, and three concurrent capture requests with no leftover device staging directories or service restarts. Typical single captures took 1.1–2.4 seconds on this tablet; concurrent requests queue per device. Evidence and receipts are in `target/screenshot-evidence/`. All 85 workspace tests and Clippy pass, including agent PNG/error/cleanup cases, physical CLI binary/JSON contracts, and an emulator QMP capture regression. Other physical orientations remain unverified; real emulator verification is recorded below. Emulator capture now also uses unique host staging directories with automatic cleanup.

## Application permissions

Permission commands use the same agent-backed system D-Bus path for physical devices and emulators. Install or upgrade the agent using `setup-device` first. They use ordinary user SSH and do not require a graphical session, Qt geometry, DRM or uinput. UID comes from the authenticated local socket peer. Permission actions run sequentially in the service, with typed D-Bus through [zbus](https://docs.rs/zbus/5.19.0/zbus/blocking/struct.Proxy.html), and no passwords after setup.

```bash
audb_app=ru.aurora.flutter_secure_storage_example
./target/debug/audb --device phone --json permission list "$audb_app"

# Grant every declared permission and explicitly allow disabling the dialog:
./target/debug/audb --device phone --json permission grant "$audb_app" --all-requested --disable-prompt

# Grant selected permissions; existing saved grants are preserved:
./target/debug/audb --device phone --json permission grant "$audb_app" UserDirs DeviceInfo --disable-prompt
./target/debug/audb --device phone --json permission revoke "$audb_app" UserDirs

# Restore the dialog and clear saved grants without clearing app data:
./target/debug/audb --device phone --json permission reset "$audb_app"
./target/debug/audb --device phone --json permission prompt "$audb_app" --enable
# Disable the dialog while preserving existing saved grants:
./target/debug/audb --device phone --json permission prompt "$audb_app" --disable
```

Use the Sailjail application ID (desktop ID), which may differ from RPM name or executable; IDs such as `jolla-settings` are valid. `list` returns `applicationId`, actual `uid`, `declared`, `granted`, boolean `showPrompt`, `mode` and optional `alwaysAllowed`. The latter is information about an OS policy, not an effective permission list. The agent checks Aurora method signatures before permitting mutations; an incompatible Sailjail API returns `CAPABILITY_UNAVAILABLE`.

Aurora toggling the dialog changes grants: switching it off auto-grants the declared set, switching it on clears grants, and writing grants while it is on silently leaves an empty set. For `grant`, an enabled dialog requires explicit `--disable-prompt`; otherwise `PROMPT_ENABLED` returns before changing state. The service snapshots the original grants, disables the dialog if needed, writes only their union with the requested permissions, then verifies the result. `prompt --disable` rewrites the original set after toggling to prevent unintended automatic grants. `prompt --enable` and `reset` explicitly clear saved grants. `--all-requested` and explicit permission names are mutually exclusive. Undeclared permissions return `PERMISSION_NOT_DECLARED` before writing.

Mutations return `before`, `after`, `changed` and `restartMayBeRequired`. Already-running applications may retain their sandbox until restarted; audb does not restart them automatically. `revoke` changes saved grants, not mandatory or global OS policy. Several setters are not atomic. A write/verification failure includes `phase`, original state and read-back state when available; unknown outcomes never trigger a replay. The permission worker is bounded to 20 seconds (including connection establishment), after which its outcome may be unknown. The socket client waits up to 25 seconds; a shorter CLI/SSH deadline does not guarantee immediate remote cancellation. Normal worker output uses a root-owned anonymous file to avoid pipe backpressure and is closed after use.

`APP_NOT_FOUND`, `PERMISSION_DENIED`, `PERMISSION_SERVICE_UNAVAILABLE`, `PERMISSION_VERIFY_FAILED`, `PROMPT_ENABLED` and `PERMISSION_NOT_DECLARED` distinguish common failures. Upgrading an older agent is required for these commands. CLI/daemon protocol is 10, local agent protocol is 3. The ordinary status capability flag indicates implementation support; permission calls probe the current system API before execution.

Verified on `phone`: list, selected grant with no unintended extra grants, merging, revoke, repeated grant with `changed: false`, prompt toggles, reset and rejection of undeclared/missing applications. After granting all permissions the installed Flutter secure-storage example reached its UI without a permission dialog. Test permissions were restored and the application stopped. Touch/screenshot regression and service readiness also passed. Permission list also succeeded with invalid Wayland/Qt display settings, confirming no geometry helper was invoked. All 85 tests and Clippy passed, covering partial writes, read-back mismatches, framing/UID restrictions, JSON errors and an emulator SSH route with no QMP. Real emulator verification is recorded below. [Detailed device behavior](research/permissions/README.md) and receipts/screenshots in `target/permission-evidence/` support these results.

## Unicode text and named keys

```bash
./target/debug/audb --device phone --json text 'Привет, Aurora! 🦀'
./target/debug/audb --device phone --json text --stdin < message.txt
./target/debug/audb --device phone --json text 'Typing slowly' --delay 50
./target/debug/audb --device phone --json key backspace
./target/debug/audb --device phone --json key enter
./target/debug/audb --device phone --json key left
```

Text inserts into the focused editor through a background Maliit QML plugin and a small C++ Qt extension loaded in the existing user keyboard process. It discovers the active stock input context through public Qt APIs and invokes its checked `sendCommit` method. It does not patch or replace the stock keyboard, alter layouts, use the clipboard, or require a root password. The private Unix socket is `/run/user/<actual UID>/audb-input.sock`, mode 0600, with peer UID checks on both ends. Normal commands send typed JSON through SSH stdin; text does not enter a remote shell command or echo back in the success result.

Default `--delay 0` commits the complete string once, independent of keyboard layout. `--stdin` preserves all UTF-8 bytes, including a trailing newline. Application input constraints still apply (single-line fields, validators, maximum length, etc.). Successful `delivery: "submitted"` confirms an IME submission, not a general read-back of application content. Limits: 4096 Unicode code points, 16 KiB, delay 0–1000 ms, planned delay at most 10 s. Control characters other than tab/newline and newline-only strings are rejected before input; use `key enter` for a standalone Return.

Paced input waits for an editor update confirming the UTF-16 cursor and expected surrounding text after each commit. This prevents a race observed with spaces and emoji in Flutter. It requires valid editor cursor feedback; otherwise use `--delay 0`. A missing text/cursor acknowledgement (1.5 s), overall deadline (11 s), changed focus or disconnected client cancels remaining input. Partial failures use `OUTCOME_UNKNOWN` and return submitted counts when the bridge can still reply. `INPUT_NOT_FOCUSED` means no characters were sent; `INPUT_BUSY` rejects a simultaneous request. Applications that switch internal fields without a distinct Maliit notification cannot be distinguished when their text/cursor state is identical; prefer the single commit mode for automation. No failed text/key action is automatically replayed. SIGINT/SIGTERM cancellation of agentctl closes its input socket within a bounded read interval; an arbitrary SSH/host deadline still cannot promise immediate remote termination.

Named keys use a second persistent uinput device in the root service. Supported names: Enter/Return, Backspace/bs, Delete/del, Escape/esc, Tab, Space, arrows, Home/End, PageUp/pgup, PageDown/pgdn, Insert, F1–F12, Shift/Ctrl/Alt (including left/right aliases), CapsLock, volumeup/vol+, volumedown/vol-, mute and power. Each command is a complete press/release click; separate modifier clicks do not implement combinations. Key release is attempted on failures/disconnects and when the service exits. A release acknowledgement reports event submission, not application acceptance.

`setup-device` restarts the selected user's running `maliit-server` to load the plugin. An application that was open during setup may need restarting once to reconnect its input context. `status` advertises `text` and `key`, and reports whether an IME editor is currently active. The physical service reports named keys even without screen geometry.

Emulators prefer the same agent path for text/keys. An unavailable read-only agent probe falls back to the existing QMP implementation; an error after dispatch never does. Explicit `--socket` selects QMP. QMP text needs a matching guest layout and supports ASCII only; a Unicode string now fails **before any ASCII prefix is typed**, rather than skipping characters. Initial routing/no-replay tests used a local SSH server and an absent QMP socket; real emulator verification is recorded below.

On KVADRA_T / Aurora 5.2.0.259, signed `audb-agent-0.3.0-10.aarch64.rpm` passed native Qt-field checks and exact JSON read-back in a temporary Flutter 3.41.4 app: Cyrillic, English, emoji, combining characters, Chinese, quoting/backslashes, tabs/newlines, paced input, both English/Russian layouts, editing keys, busy rejection, focus cancellation and client SIGTERM. Receipts are in `target/text-evidence/`; [implementation research and reproduction](research/text-input/README.md) explain the tested scope. All 85 workspace tests and Clippy pass. Other Aurora releases and device configurations still require verification.

## Real emulator verification

On 2026-10-08, the real SDK emulator AuroraOS-5.2.1.200 (x86_64, guest UID 100000) passed the common CLI scenario with signed `audb-agent-0.3.0-10.x86_64.rpm`. The service package was built using `scripts/build-agent-rpm.py --arch x86_64 --rust-target x86_64-unknown-linux-gnu --target AuroraOS-5.2.1.200-x86_64` and installed with:

```bash
./target/debug/audb --device emulator --json setup-device \
  --rpm target/agent-rpm/x86_64/signed/audb-agent-0.3.0-10.x86_64.rpm
./target/debug/audb --device emulator --json doctor
```

The x86_64 system RPM carries a Regular development signature, whose CA trust was not independently verified. Its system locations and scripts do not pass the Regular application validator; setup uses the already agreed developer system-package installation route with the transaction validation override. The temporary Flutter application RPM retained its existing Regular signature and passed direct Regular validation.

Verified: `doctor`/`capabilities`, nonempty permission grants (`UserDirs`), repeat grant with no changes, revoke/reset and disabling the prompt before launching a freshly installed Flutter fixture; exact editor read-back of Cyrillic, emoji, combining accents, Chinese, tabs/newlines/quotes/backslashes; paced text at 30 ms; Backspace/Left/Delete/Enter; QMP taps and edge-up swipe to home; reopening the running application by tapping its cover; PNG file output and raw stdout. Capture preserved editor content and focus. The screenshot backend was QMP (360×800), text Maliit, and named keys uinput. `doctor` also passed after screenshots and with a focused editor.

Emulator QMP tap/swipe now read the primary display's current PNG dimensions before every gesture. Coordinates use that display's pixels, with bounds checks and proportional direction/edge aliases; invalid coordinates or unavailable geometry send no touch events. Only the PNG header/trailer are loaded into memory, and the transient host dump is removed. This VM reports 360×800 through QMP; its agent Qt geometry of 720×1600 belongs to a different coordinate space. Real VM taps/swipes passed; portrait, landscape, resize between gestures and out-of-bounds cases passed QMP socket tests. Changing the real VM resolution remains unverified. `app launch` starts an application; RuntimeManager can return ApplicationAlreadyRunning for an existing process. Use its home cover to bring that window forward. A running PID alone does not mean the first UI frame has rendered.

The earlier fixture installation used `push` plus APM over `shell`. The updated `package install` installed the same 34,196,365-byte RPM directly on this VM, verified its APM registration and cleaned staging. A repeated install returned `alreadyInstalled: true` and `changed: false`. New receipts and gesture screenshots are in `target/limits-evidence/`; all 91 workspace tests and Clippy pass. The broader file/command parity audit remains open.

Receipts, the reproducible temporary harness, screenshots and SDK logs are in `target/emulator-evidence/`. All 85 workspace tests and Clippy pass. The QMP single-client regression is covered by a sequential mock endpoint; the registry test now uses an isolated absent socket instead of connecting to a real host emulator. The temporary app/RPM were removed after testing. The emulator remains running with audb-agent installed.

## Readiness and troubleshooting

From this checkout, use the freshly built CLI (the globally installed `audb` may be older):

```bash
./target/debug/audb --device phone --json doctor
./target/debug/audb --device phone --json capabilities
```

Both commands inspect the registered target without entering text, changing permissions or restarting services. Emulator geometry inspection creates a transient QMP screenshot, reads its dimensions and removes it; physical inspection uses agent geometry. Checks have individual deadlines of 4–10 seconds; `--command-timeout` still bounds the entire request including queue wait. They never request a root password or run setup automatically. A configured root SSH account is checked noninteractively with `id -u`.

`doctor` returns `data.reportVersion: 1`, device/version metadata, `healthy`, `checks` and `capabilities`. Checks cover SSH and UID/architecture, SFTP and OS version, the graphical user bus, agent protocol/version and geometry, Maliit focus, Sailjail permission API/signatures, MCE, RuntimeManager, APM, configured root SSH and emulator QMP. Each check has a stable name, `status` (`passed`, `failed`, `warning`, `skipped`), `code`, `message`, `fix` and `data`. Dependent checks are skipped after SSH failure; QMP is checked independently. QMP probes release the idle cached connection first and use a disposable connection for status, command inventory and geometry capture. This avoids hanging on QEMU endpoints that accept one client.

`healthy` means no executed check failed; warnings, unconfigured optional root SSH and intentionally unprobed operations do not make it false. A successfully collected report returns exit code 0 and outer `ok: true`, even when `healthy: false`. Agents must inspect the report. Argument/registry errors and failure to obtain a report use the usual nonzero error contract.

`capabilities` returns the same `reportVersion`, device metadata and capability map, omitting the detailed checks. Each capability has:

- `supported`: whether audb implements it for this target kind.
- `available`: whether its backend responded; `null` means not probed.
- `ready`: whether known current prerequisites are satisfied; `null` means not probed.
- `backend`, `reasonCode`, `reason`, `fix`, `limitations`: routing information, an explanation and the next action.
- `geometry`: QMP width/height, source and coordinate space for emulator tap/swipe/screenshot; otherwise `null`.

For example, a working Maliit backend with no focused editor reports `text: {supported: true, available: true, ready: false, backend: "maliit", reasonCode: "INPUT_NOT_FOCUSED", ...}`. Tap/swipe additionally require screen geometry, an awake/unlocked session and enabled touch policy. Emulator tap/swipe/screenshots use QMP and expose `coordinateSpace: "qmp-primary-display"`; unavailable touch geometry reports `GEOMETRY_UNAVAILABLE`. Text/key prefer an advertised agent backend just like actual dispatch. QMP text is explicitly marked ASCII-only and depends on the guest layout.

`displayStatus` and `displayControl` are separate: reading MCE may work while the current control implementation requires unavailable root SSH. Current `logs` also requires root SSH and journalctl. App/package listing and an SFTP read of `/etc/os-release` are probed; readiness does not guarantee an arbitrary app, path, RPM or installation will pass its specific policies. Application RPM installation streams files over SFTP. Info, perf, crash, sandbox, network, URL opening, system package installation and emulator sensor/location APIs are marked `NOT_PROBED` with nullable availability/readiness. Clipboard and UI tree are explicitly unsupported.

These are point-in-time reports. Check the actual command result even after `ready: true`; a failed modifying command is never replayed based on a readiness report.

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
{"ok":true,"schemaVersion":1,"deviceId":"phone","data":{}}
```

Failure:

```json
{"ok":false,"schemaVersion":1,"deviceId":"phone","error":{"code":"CAPABILITY_UNAVAILABLE","message":"..."}}
```

`deviceId` is the resolved target, or `null` for untargeted registry operations and errors before target resolution. Shell output preserves whitespace in both JSON and plain modes. Remote command failures include the exit code, stdout and stderr in `data`, and return a nonzero CLI exit status. Transport failures after possible dispatch return `OUTCOME_UNKNOWN`; modifying commands are never automatically repeated.

The public process starts a private daemon mode automatically. Each device has an independent runtime and operation queue; different targets can execute concurrently. Emulator SSH and QMP sessions are retained. Physical operations currently launch OpenSSH processes per invocation, retaining profile and authentication behavior. Registration updates recreate a runtime on its next request, and queued requests recheck the registry before execution. A watch/monitor still occupies its own device queue; freeing that queue between polls remains planned.

`--command-timeout SECONDS` bounds daemon execution including queue wait (default 300, maximum 86400). A timeout can leave the remote result unknown and does not cause a retry. CLI/daemon protocol version 10 uses a separate socket from older releases.

`package install ./app.rpm --timeout 60` works on phones and emulators. The CLI sends the absolute local path to the daemon; SFTP streams the RPM into a private device staging directory without a JSON byte array or whole-file buffering. audb checks RPM metadata/architecture and calls ordinary-user APM installation with the existing OS validation policy. `--timeout` (1–3600 seconds, default 60) bounds the subsequent wait for matching package ID and version through `APM.GetPackage`; the global command deadline also includes upload and queue wait. A matching installed version returns `verified: true`, `alreadyInstalled: true`, `changed: false` without another installation.

Installation is requested once. Success cleans staging and reports `stagingCleanup`; a failure before dispatch also attempts cleanup. If APM may have received the request but its outcome is uncertain, audb retains the staged RPM for the asynchronous installer, returns an error and never repeats installation automatically. Completed errors include `stagingRetained` and `stagingDirectory`; outer cancellation may return only `OUTCOME_UNKNOWN`. Retained `/tmp/audb-app-rpm.*` directories can be removed after checking the final installation state.

## Commands

```text
tap, swipe, text, key, screenshot, status, doctor, capabilities
permission list|grant|revoke|reset|prompt
install, uninstall, setup-status, setup-device
emulator start|stop|status
device add|update|remove|list|current, select, setup-root
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
package list|install|install-system|uninstall|sign|validate
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

- Target modifying commands explicitly with `--device` when multiple devices are registered.
- Sandbox paths are canonicalized in the guest and cannot escape an application's private roots.
- SQLite accepts only read-only query prefixes and limits output to 1000 rows.
- `app clear-data` requires either `--dry-run` or explicit `--confirm`.
- `logs --clear` requires `--force`.
