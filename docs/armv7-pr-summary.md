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
| `c63d0c9` | Packaging derives arch and libdir from the sb2 target; ELF/payload assertions; linker machine guard |
| `56b02fb` | Compile-time pin of the 32-bit uinput event layout |
| `45f4103` | Implementation status and verification evidence in the design doc |

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
(`%{_target_cpu}` or `%{_arch}`), with tokens validated as `[A-Za-z0-9_]+`. No arch alias
exists, nothing rewrites `arm` into `armv7hl`, and no rule can equate a 32-bit ARM name with
`aarch64`. Keeping `%{_arch}` in the accepted set means no device that installs today can
start failing. The pre-check remains deliberately narrower than rpm's `arch_compat` chain;
the transaction stays the authority.

## Reviewed

Design reviewed with fresh, read-only context by the architect (Sol 6.1) and independently
challenged by the architecture critic (per-call `cursor/grok-4.7`). Both reviews are committed
verbatim; §5 of the design doc dispositions every must-fix. The critic's "no alias; exact
match only" and the architect's "query the package namespace" instructions are both followed.
One deliberate, disclosed deviation: the design accepts **any** rpm-reported platform arch by
exact match (non-regressive) instead of preferring `_target_cpu` and falling back — the
preferred-order variant needs the aarch64 `%{_target_cpu}` observation, and it would newly
reject plain-`arm` packages that pass the gate today.

## Verified here

- `cargo test -p audb-protocol`: 7 pass.
- `cargo test -p audb-core`: 51 pass, 1 fail — `setup::tests::migrates_audb2_wrapper_in_place`
  (`setup.rs:303`) is **pre-existing and macOS-specific**: it asserts a `/tmp/` path while
  macOS tempdirs live under `/var/folders/…`. It fails identically on the base commit.
- `cargo clippy -p audb-core -p audb-protocol -p audb-client --all-targets`: clean.
- `cargo clippy -p audb-agent --target armv7-unknown-linux-gnueabihf --all-targets`: clean.
  (`audb-agent` is Linux-only — `libc::SO_PEERCRED`/`ucred` — so it cannot be built or linted
  for the macOS host at all; the armv7 target is the correct way to check it.)
- `cargo check -p audb-agent --target armv7-unknown-linux-gnueabihf`: clean; the compile-time
  `InputEvent` size assertion holds on 32-bit ARM (confirmed by perturbing the expectation).
- `rustc --print cfg --target armv7-unknown-linux-gnueabihf`: `target_abi="eabihf"`,
  `target_pointer_width="32"`, no `feature="neon"` ⇒ no `-C target-feature=-neon` needed.
- `scripts/build-agent-rpm.py --self-test` and `sh -n scripts/aurora-linker.sh`: ok.
- **On-device** (`t1100` registered, read-only doctor, evidence in
  `target/armv7/evidence/doctor-t1100-private.json`):
  `identity` reports `architecture: aarch64`, `longBit: 32`, `rpmArch: armv7hl`,
  `rpmArchClass: arm`, `rpmLibdir: /usr/lib`, and a new `architectureBitness` **warning**
  (`MIXED_KERNEL_USERSPACE_BITNESS`). `healthy: false` only because `audb-agent` is not yet
  installed on the device.

## Witnessed on device

Evidence files (gitignored, host-local): `target/armv7/evidence/`
`doctor-t1100-private.json`, `verification-battery.txt`.

## Not verified — blocked (single blocker)

**The OrbStack docker daemon is wedged**, so the SDK container cannot be used. Symptoms:
`curl --unix-socket ~/.orbstack/run/docker.sock http://localhost/_ping` alternates between
timeout and `OK`; `docker ps`/`inspect`/`top` intermittently hang; `docker exec <container>`
times out; `docker restart <container>` never took effect (container still `Up 2 days`);
OrbStack helpers 32420/32427 alive. It broke right after a `find /` exec inside the container
filled the 180 s run and timed out. Restarting OrbStack would also stop the unrelated
`vikunja`/`vikunja-t10` containers, so it was not done without approval.

Consequently still outstanding:

1. the armv7hl sb2 target's existence and hard-float proof (`__ARM_PCS_VFP`, not `__SOFTFP__`),
   Qt5 dev files and `moc`, and the target's `%{_libdir}`;
2. the aarch64 sb2 target's `%{_target_cpu}`/`%{_libdir}` cross-check (a `%{_libdir}` spec
   change is an aarch64 change too);
3. the armv7hl RPM build and its `%{ARCH}`/payload assertions;
4. `setup-device --check-only` against t1100, then install + service restart;
5. `doctor` / one non-touch command after installation.

## Other findings

- **The client integration suite is destructive on macOS.** `audb-client/tests/devices.rs`
  isolates with `XDG_CONFIG_HOME`, but state resolves through `directories::BaseDirs`
  (`audb-core/src/config.rs:2,55-59`), which ignores XDG on macOS, so all 18 tests share the
  real registry. A baseline run appended a `phone` device and rewrote `emulator.qmpSocket`; the
  registry was restored byte-exactly and a pre-restore copy kept at
  `~/Library/Application Support/audb/devices-v1.json.pre-armv7-testrun.bak`. `phone` later
  reappeared (a sibling worktree, `audb-fbdev-touch`, runs daemons and tests against the same
  registry), so it was left untouched this time rather than edited outside this branch's scope.
- `setup-device` still defaults to `packages/audb-agent.rpm` (`audb-client/src/main.rs:658`);
  the armv7hl file must be passed explicitly. Change not made deliberately.
- Unsigned RPM install path unchanged (`system_package.rs:260-266`, no `--nosignature`); if
  t1100 enforces signatures the install will fail and signing needs separate approval.
- Device is Aurora 5.2.0.180 while the SDK target is 5.2.1.200: same ABI family, runtime
  dependency differences unverified.
