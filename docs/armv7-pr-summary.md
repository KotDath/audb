# PR summary — audb armv7 (Fplus T1100) support

Branch `audb-armv7` based on `origin/main` `190bbaa`. Local commits only; nothing pushed.

## Objective

Make `audb` work end-to-end on the Fplus T1100, which runs an aarch64 kernel with a 32-bit
ARM (`armv7hl`) userspace. Before this branch the agent could not be installed at all: the
package architecture gate compared an RPM against an architecture rpm never reports for the
device.

## Commits

| Commit | Change |
| --- | --- |
| `e01efad` | Design of record (`docs/armv7-support.md`) plus the verbatim independent reviews (`docs/reviews/`) |
| `af52ec0` | Shared RPM-platform arch probe/parse/gate used by both install paths |
| `1f77fbc` | Doctor reports rpm arch, rpm libdir and userspace bitness; bitness warning |
| `c63d0c9` | Packaging derives arch and libdir from the target rpm; ELF/payload assertions; linker machine guard |
| `56b02fb` | Compile-time pin of the 32-bit uinput event layout |
| `45f4103`, `2682339`, `dbaf58c` | Implementation status, PR summary, and measured SDK/build evidence |
| `a051558` | Drive the container's real cross toolchain (`sb2` there is a shim) |

## Root cause (measured, not inferred)

| Probe on t1100 | Value |
| --- | --- |
| `rpm --eval '%{_arch}'` | `arm` ← what the gate asked for |
| `rpm --eval '%{_target_cpu}'` | `armv7hl` ← the actual package namespace |
| installed RPM arch census | **940 × armv7hl**, 211 × noarch, 0 others |
| `/etc/rpm/platform` | `armv7hl-meego-linux` |
| `%{_libdir}` | `/usr/lib` (and `/usr/lib64/qt5/qml` does not exist) |
| loader / bitness / libc | `/lib/ld-linux-armhf.so.3`, `LONG_BIT=32`, glibc 2.38 |

`%{_arch}` is rpm's coarse family label, not the namespace packages are built for. The fix is
to observe the right value, not to alias one string onto another: the gate now accepts
`noarch` or an **exact** match against any architecture the device's own rpm reports
(`%{_target_cpu}` or `%{_arch}`), tokens validated as `[A-Za-z0-9_]+`. No alias exists, nothing
rewrites `arm` into `armv7hl`, and no rule can equate a 32-bit ARM name with `aarch64`.

The aarch64 side was measured on `t1` (operator-approved, read-only): `%{_target_cpu}` and
`%{_arch}` are both `aarch64` and `%{_libdir}` is `/usr/lib64`, so the accepted set on a
working 64-bit device is unchanged — the fix is provably non-regressive.

## Reviewed

Design reviewed with fresh, read-only context by the architect (**requested `openai-codex/gpt-6.1-sol` = Sol 6.1, executed `gpt-6.1-sol`**) and independently challenged by the architecture
critic (**requested per-call `cursor/grok-4.7`, executed `grok-4.7`**). Both reviews are
committed verbatim; §5 of the design doc dispositions every must-fix (no alias, query the
package namespace, float ABI on all four ELFs, target-derived libdir, `--arch` assertion-only,
doctor warning not failure, fake-`rpm` shell test). One deliberate, disclosed deviation: the
gate accepts **any** rpm-reported platform arch by exact match (non-regressive) instead of
preferring `_target_cpu` with a fallback — now moot, since `t1` proved both macros agree on
aarch64.

## Verified here

- `cargo test -p audb-protocol`: 7 pass. `cargo test -p audb-core`: 51 pass, 1 fail —
  `setup::tests::migrates_audb2_wrapper_in_place` (`setup.rs:303`) is **pre-existing and
  macOS-specific** (asserts a `/tmp/` path; macOS tempdirs are under `/var/folders/…`); it
  fails identically on the base commit.
- clippy clean for host crates, and for `audb-agent --target armv7-unknown-linux-gnueabihf`
  (the agent is Linux-only, so `--workspace` cannot be green on macOS at all).
- `rustc --print cfg --target armv7-unknown-linux-gnueabihf`: `target_abi="eabihf"`, 32-bit, no
  `feature="neon"`; the compile-time `InputEvent` size assertion holds and fires when perturbed.
- packaging `--self-test`, `py_compile`, `sh -n`: ok.
- **armv7hl RPM built**: `target/agent-rpm/armv7hl/audb-agent-0.3.0-10.armv7hl.rpm`,
  `rpmArchHeader=armv7hl`, payload under `/usr/lib` with no `lib64` entries, receipt at
  `target/agent-rpm/armv7hl/build-receipt.json`.
- **Payload independently verified** (extracted with `rpm2cpio`): all four ELFs are ELF32/ARM
  with hard-float ABI flags and `Tag_ABI_VFP_args: VFP registers`; the three executables
  request `/lib/ld-linux-armhf.so.3`; max symbol floor `GLIBC_2.34` against the device's 2.38;
  the device has libQt5Core/Gui/Quick/Qml/Network.so.5, `libstdc++`, `libgcc_s`, and the **same
  Qt 5.6.3** as the SDK sysroot.
- **aarch64 non-regression**: `--platform aarch64` still builds `ARCH=aarch64` with the
  identical `/usr/lib64` payload layout.
- **Gate proven on the real device**: with a deliberately wrong `--root-password-stdin`,
  `setup-device --check-only` fails at `phase: "check"` with devel-su `Auth failed` while
  carrying `package.arch = armv7hl` — i.e. it passed the new gate (the old gate refused
  `armv7hl` before any root access). Staging cleaned, nothing installed.
  Evidence: `target/armv7/evidence/gate-probe-wrong-password.json`.
- **On-device read-only doctor**: `architecture: aarch64`, `longBit: 32`, `rpmArch: armv7hl`,
  `rpmArchClass: arm`, `rpmLibdir: /usr/lib`, plus the new `architectureBitness` warning.
  Evidence: `target/armv7/evidence/doctor-t1100-private.json`, `verification-battery.txt`.

## Working end-to-end on t1100

After signing the armv7hl RPM with the Regular development certificate (`audb package sign`)
and reinstalling with `--reinstall`:

| Evidence | Result |
| --- | --- |
| `rpm -q --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}'` | `audb-agent-0.3.0-10.armv7hl` |
| `rpm -ql` | Qt/Maliit paths under `/usr/lib` (libdir fix on real hardware) |
| `systemctl status audb-agent` | **`active (running)`**, `/usr/sbin/audb-agent` |
| `rpm -V audb-agent` | clean — installed files match the package |
| `getfattr` on all four payload files | `security.appid="audb-agent"` + `security.ima=0s…` |
| `audb doctor` | **`healthy: true`**, `agent: passed` (backend `uinput`), `agentRpm 0.3.0-10`, identity reports `rpmArch armv7hl`, `rpmArchClass arm`, `rpmLibdir /usr/lib`, `longBit 32`, `architecture aarch64`; `architectureBitness` stays a warning |
| non-touch command | `audb --device t1100 screenshot` → real PNG **2176×1600, 6336658 bytes** |

`tap`/`swipe` report `available=false` while the screen is off and locked
(`DISPLAY_NOT_READY`) — documented readiness behaviour; touch/geometry verification belongs to
branch U.

### Why signing is mandatory here (finding)

The unsigned package installed fine but its files could not execute. The device boots with
`ima_appraise=enforce` and ships `rpm-plugin-external-ima`, which applies
`security.ima`/`security.appid` from the package's **external signature**; unsigned, there is
nothing to apply, so IMA appraisal denies execution (`203/EXEC`, loader `failed to map segment
from shared object`, `Permission denied`, exit 126) and the install warns
`Plugin sweeper: hook tsm_post failed`. After signing, all four files are attributed and the
service runs.

`rpmkeys --checksig` reports `Payload … digest: BAD` for externally signed packages — on the
device's own rpm as well as in the SDK, for aarch64 too and with both build-tools images — so
that is a property of Aurora's external-signature layout, not of this build. The freshly built
unsigned RPM verifies cleanly, and the build script now enforces exactly that (plus a
container-vs-host digest comparison) before anything can sign or ship it.

## Other findings

- **The client integration suite is destructive on macOS.** `audb-client/tests/devices.rs`
  isolates with `XDG_CONFIG_HOME`, but state resolves through `directories::BaseDirs`
  (`audb-core/src/config.rs:2,55-59`), which ignores XDG on macOS, so all 18 tests share the
  real registry. A baseline run appended a `phone` device and rewrote `emulator.qmpSocket`; the
  registry was restored byte-exactly and a pre-restore copy kept at
  `devices-v1.json.pre-armv7-testrun.bak`. Per the operator, that defect is resolved in the
  `audb-fbdev-touch` worktree, so this branch deliberately does **not** touch `config.rs`.
- `setup-device` still defaults to `packages/audb-agent.rpm` (`audb-client/src/main.rs:658`);
  the armv7hl file is passed explicitly by design.
- Unsigned RPM install path unchanged (`system_package.rs:260-266`, no `--nosignature`).
- The container's `/usr/local/bin/sb2` is a 149-byte shim that discards `-t`, so prior "sb2
  target" builds were silently native; the packaging path now selects a platform explicitly.
