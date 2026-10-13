# audb — 32-bit ARM (armv7hl) support design

**Status: design of record — reviewed by the primary architect (Sol 6.1) and independently
challenged by the architecture critic (cursor/grok-4.7).** Both reviews are archived verbatim
in `docs/reviews/`. Review dispositions are in §5. No production code was written before the
reviews returned.

Goal: make `audb` (client + agent + packaging) work end-to-end on the Fplus T1100 — an
`aarch64` kernel with 32-bit ARM userspace. Branch `audb-armv7`, base `origin/main` @
`190bbaa`, local commits only.

---

## 1. Measured evidence

All device commands were read-only; nothing on the device was modified.

### 1.1 Device `t1100` (host alias in `~/.ssh/config` → 192.168.1.207)

| Property | Value | Command |
| --- | --- | --- |
| `uname -m` / kernel | `aarch64` / `4.14.186` | `uname -a` |
| userspace bitness | `32` | `getconf LONG_BIT` |
| libc | `glibc 2.38` | `getconf GNU_LIBC_VERSION` |
| OS / rpm | Aurora `5.2.0.180 (Khabarovsk)` release / `4.16.1.3` | `/etc/os-release`, `rpm --version` |
| `%{_arch}` | **`arm`** ← what the gate queries today | `rpm --eval '%{_arch}'` |
| `%{_target_cpu}` | **`armv7hl`** ← the package namespace | `rpm --eval '%{_target_cpu}'` |
| `%{_target_platform}` | `armv7hl-meego-linux` | `rpm --eval …` |
| `/etc/rpm/platform` | `armv7hl-meego-linux` | `cat /etc/rpm/platform` |
| `%{_host}` / `%{_host_cpu}` | `armv7l-unknown-linux-gnueabi` / `armv7l` | `rpm --eval …` |
| `%{_libdir}` / `%{_lib}` | `/usr/lib` / `lib` | `rpm --eval …` |
| **installed RPM arch census** | **940 × `armv7hl`, 211 × `noarch`, 0 others** | `rpm -qa --qf '%{ARCH}\n' \| sort \| uniq -c` |
| float ABI | `/lib/ld-linux-armhf.so.3` present; `ld-linux-aarch64.so.1` absent | `ls -l /lib/ld-linux*.so.3`; `test -e …` |
| rpm binary itself | `ELF 32-bit LSB pie, ARM, EABI5, interpreter /lib/ld-linux-armhf.so.3` | `file /usr/bin/rpm` |
| optflags in effect | `-O2 -g -march=armv7-a -mfloat-abi=hard -mfpu=vfpv3-d16` | `rpm --eval '%{optflags}'` |
| rpm arch canon / compat | `arch_canon: armv7hl: armv7hl 12`; `arch_compat: armv7hl: armv6hl`, `armv6hl: noarch` — **no `armv7hl: arm` edge** | `grep -i arm /usr/lib/rpm/rpmc` |
| QML / Maliit dirs that exist | `/usr/lib/qt5/qml`, `/usr/lib/maliit/plugins` | `ls -d …` |
| 64-bit dirs | `/usr/lib64` exists but `/usr/lib64/qt5/qml` and `/usr/lib64/maliit` do **not** | `ls -ld …` |
| systemd unit dir | `/usr/lib/systemd/system` | `ls -d …` |
| audb installed | none | `rpm -qa \| grep audb` |
| device tooling | `file`, `ldd` present; **`readelf`, `objdump` missing** | `command -v …` |
| resources | `/` 86% used (658 MB free), 3.7 GiB RAM | `df -h /` |

**Provenance, observed rather than inferred.** `%{_arch}` resolves to `arm` while
`%{_target_cpu}` resolves to `armv7hl`, and `/etc/rpm/platform` is
`armv7hl-meego-linux`. `/usr/lib/rpm/macros` contains the literal lines `%_host_cpu armv7l`
and `%_target_cpu %{_host_cpu}` (lines 1020-1026), which would expand to `armv7l` — yet the
observed `%{_target_cpu}` is `armv7hl`. The effective value therefore comes from the
platform configuration (`/etc/rpm/platform`) and takes precedence over that macro-file
default. We record the observed values and do **not** derive `_target_cpu` from `_host_cpu`:
`armv7l` is not an installed package architecture (0 of 1151 packages).

### 1.2 Host / repo

- `rustc`/`cargo` 1.97.1. Installed targets: `aarch64-apple-darwin`,
  `aarch64-unknown-linux-gnu`. `armv7-unknown-linux-gnueabihf` is listed by
  `rustc --print target-list` but **not installed** (Phase 2 installs it; toolchain state is
  never committed).
- Release profile: `opt-level="z"`, `lto=true`, `codegen-units=1`, `strip=true`,
  `panic="abort"`.
- `audb-agent`'s direct dependency graph is pure Rust (`libc`, `tempfile`, `crc32fast`,
  `serde`, `serde_json`, `audb-protocol`, `zbus`, `roxmltree`); no repo `build.rs`, no
  `cc`/`-sys` crate. `audb-core`'s bundled `rusqlite` is not in the agent graph.
- The release artifact is **four ELF files**, not just Rust binaries: `audb-agent`,
  `audb-agentctl`, the Qt helper `usr/libexec/audb-agent/display`
  (`scripts/build-agent-rpm.py:51`) and the Maliit plugin
  `…/libaudbinput.so` (`:61`). The two C++ objects are compiled by sb2 `g++` and never pass
  through `scripts/aurora-linker.sh` (`:3-4`).

### 1.3 SDK container — still blocked at design time

`docker` (OrbStack) stopped answering mid-session (`docker version`, `docker ps`, and a raw
`curl --unix-socket ~/.orbstack/run/docker.sock http://localhost/_ping` all time out; via the
socket the ping times out after 12 s). It had answered earlier in the session; a `find /`
exec inside the container filled the run and timed out just before the socket went
unresponsive. OrbStack helper PIDs 32420/32427 remain alive. **Restarting OrbStack would stop
the unrelated `vikunja`/`vikunja-t10` containers, so it was not done — operator approval
requested.**

Therefore, at design time the following are **unmeasured** and gate the packaging work:

1. that sb2 target `armv7hl-meego-linux-gnueabi` exists and is usable (an early probe printed
   `aarch64-meego-linux-gnu`, which is the *aarch64* target's value — a failed measurement,
   not evidence);
2. `gcc -dumpmachine` and the target's default float ABI (`__ARM_PCS_VFP` vs `__SOFTFP__`);
3. Qt5 dev files (`Qt5Gui/Qt5Core/Qt5Quick/Qt5Qml/Qt5Network`) and `moc` for that target;
4. `rpm --eval '%{_target_cpu} %{_arch} %{_libdir}'` **inside that target**;
5. the effective rpmbuild output directory.

The float-ABI question is independently answered by the device (§1.1), so (2) only decides
whether explicit `-march=armv7-a -mfloat-abi=hard -mfpu=vfpv3-d16` must be passed to the
`g++` and link commands.

---

## 2. Root cause (line numbers verified on `190bbaa`)

1. **The arch gate asks for the wrong rpm namespace.**
   `audb-core/src/system_package.rs:152-155` and `audb-core/src/system.rs:233-235` both run
   `rpm --eval '%{_arch}'` and require `package.arch == that` (or `noarch`). On t1100 that
   compares `armv7hl` (package) with `arm` (family label) → every real package is refused.
   This is not a missing alias: the observation is wrong.
2. **Doctor reports only `uname -m`.** `readiness.rs:12-21` (`INVENTORY`) → `architecture`
   is `aarch64` on a 32-bit userspace; required-field check `:149-153`; surfaced in `identity`
   at `:320`.
3. **Packaging hardcodes 64-bit paths.** `packaging/audb-agent.spec:45-46`, staging at
   `scripts/build-agent-rpm.py:54,57`; the single `--arch` value drives the work dir, rpmbuild
   topdir, RPM filename and `RPMS/<arch>` path (`:34,72,80,82`). `spec:47`
   (`/usr/lib/systemd/system`) is correct and is **not** `%{_libdir}`.
4. Only the aarch64 Rust target is installed on this host.

---

## 3. Design of record

### D1 — ABI: armv7hl, EABI5 hard-float

- Rust target **`armv7-unknown-linux-gnueabihf`**. Mer's `armv7l-unknown-linux-gnueabi` tuple
  is a vendor string, not the ABI: the device runs `/lib/ld-linux-armhf.so.3` with
  `-mfloat-abi=hard -mfpu=vfpv3-d16`. A soft-float Rust target would ask for
  `/lib/ld-linux.so.3` and is wrong.
- `libc::timeval` is 8 bytes on 32-bit ARM and 16 on aarch64, so the `#[repr(C)] InputEvent`
  in `audb-agent/src/bin/audb-agent.rs:26-31` (16 bytes on this triple) matches what an
  aarch64 kernel's compat `input_event` expects. A `cfg`-gated compile-time assertion pins
  this (see §4.4).
- Float ABI is a property of **every shipped ELF**. If the SDK target's `g++` is soft-float,
  `-march=armv7-a -mfloat-abi=hard -mfpu=vfpv3-d16` must be added to both `g++` commands
  (`build-agent-rpm.py:51,61`) *and* to the Rust link line (`-C link-arg=…`, not a bare
  `RUSTFLAGS` flag, so the gcc driver applies it). A soft-float-only sysroot cannot be
  repaired with flags and must be reported instead.
- Proof required after the build, on the **staged/post-strip** files, via `readelf` inside
  sb2 (neither the device nor the macOS host is assumed to have it): `Tag_ABI_VFP_args: VFP
  registers` on all four ELFs, and `PT_INTERP` = `ld-linux-armhf.so.3` on the two executables
  (a shared object has no interpreter). `e_flags` bit `0x400` is explicitly **not** used as
  the proof.
- Also verify the Rust target's FPU baseline is not `-neon`
  (`rustc --print cfg --target armv7-unknown-linux-gnueabihf`; add `-C target-feature=-neon`
  only if needed) and check `readelf --version-info` for GLIBC/GLIBCXX/CXXABI requirements
  against the device's glibc 2.38. The "glibc 2.17 baseline" of the Rust target is *not*
  asserted as fact for rustc 1.97.1 — the built binary is the evidence.

### D2 — Arch gate: one probe, validated tokens, exact match only

**Probe (one remote command, shared by both install paths):**

```
rpm --eval '%{_target_cpu} %{_arch}'
```

**Parse (in Rust, not in shell — policy must be testable):** split on whitespace, trim, keep
tokens matching `^[A-Za-z0-9_]+$`, drop empties and any token containing `%`/`{`/`}` (rpm
echoes unresolved macros verbatim). No usable token → `CoreError::runtime` ("device did not
report an RPM architecture"). No shell `case` fallback: the existing mocks only match
`rpm --eval` and return a single token, so hidden shell policy would be untestable.

**Accept** `package.arch` when it is `noarch` or is **exactly equal** to any observed token.
Otherwise reject with the existing invalid-package error category, naming the package arch and
every observed token.

Why a union of the observed tokens rather than "prefer `_target_cpu`, else `_arch`":
the union is **non-regressive by construction** — `_arch` (today's only accepted value) stays
accepted, so no device that installs today can start failing, while `_target_cpu` equality is
what makes t1100 work. It adds **no alias and no rewriting**: every accepted value is a direct
rpm statement about this machine, and equality is per-token exact, so nothing in the rule can
ever equate `arm`/`armv7hl` with `aarch64`. (Preferred-order was the reviewers' recipe; it is
narrower still but requires confirming `%{_target_cpu}` on a working aarch64 device before it
is safe, and it would newly reject plain-`arm` packages that the gate accepts today — see §5
disposition R3.)

This pre-gate is deliberately **stricter than rpm**, which would also accept `armv6hl` via
`arch_compat`. That is intentional: it is a UX pre-check. The transaction remains the
authority (`rpm --test` on the system path, APM on the other), and `rpm_command`
(`system_package.rs:260-266`) passes no `--ignorearch`.

Observed consequences: on t1100 tokens are `{armv7hl, arm}` → our `armv7hl` RPM is accepted;
an `aarch64` package is rejected. On an aarch64 device tokens are `{aarch64}` (or
`{aarch64, aarch64}`) → `armv7hl` is rejected.

### D3 — libdir: derive from the target that runs rpmbuild, then assert the device contract

- `packaging/audb-agent.spec:45-46` → `%{_libdir}/qt5/qml/Audb` and
  `%{_libdir}/maliit/plugins/zz-audb-input.qml`. `spec:47` stays literal.
- `scripts/build-agent-rpm.py` probes `rpm --eval '%{_target_cpu} %{_arch} %{_libdir}'` **in
  the selected sb2 target** (the same rpm that runs rpmbuild) and uses the resulting libdir
  for the staging path — stripping the leading `/` before joining it onto the staging dir, so
  a target `_libdir` can never escape the staging tree. `%{_libdir}` is an install path inside
  the payload and is never used as a host or sb2 filesystem path.
- `--arch` becomes **assertion-only** (it may fail a build, never choose a path); it must not
  override the probe. The effective rpmbuild context is discovered, not predicted: the topdir
  is wiped before the build and the single produced RPM is found by glob, then asserted with
  `rpm -qp --qf '%{ARCH}'` and `rpm -qpl` (Qt/Maliit paths under the probed libdir,
  systemd path still `/usr/lib/systemd/system`).
- The build **fails closed** if the probe disagrees with the device contract for that arch:
  `armv7hl` ⇒ `/usr/lib`, `aarch64` ⇒ `/usr/lib64` (the aarch64 side is confirmed by probing
  the aarch64 sb2 target before the spec edit, since editing `%{_libdir}` is an aarch64
  change too).
- The same NVR (`audb-agent-0.3.0-10`) is kept across architectures; the built file is
  `audb-agent-0.3.0-10.armv7hl.rpm`.

### D4 — Doctor: additive, diagnostic only

`INVENTORY` (`readiness.rs:12-21`) gains, under the existing `key\tvalue` protocol and inside
the existing `command -v rpm` guard:

```
rpmArch       <- rpm --eval '%{_target_cpu}'   (empty when rpm is absent)
rpmArchClass  <- rpm --eval '%{_arch}'
rpmLibdir     <- rpm --eval '%{_libdir}'
longBit       <- getconf LONG_BIT
```

- `architecture` (uname) stays required and unchanged; all new fields are optional, so
  `readiness.rs:653-656` and the client fixture `audb-client/tests/devices.rs:1025` remain
  valid.
- Acquisition failure yields an absent field — it never fails the SSH inventory.
- The new fields are added to the `identity` check payload (`readiness.rs:320`) so
  `armv7hl`/`/usr/lib`/`32` are visible next to `architecture: aarch64`.
- New **warning** check (status `warning`, which `readiness.rs:352` excludes from `healthy`)
  when `longBit == 32` and `uname -m` is a 64-bit machine name (`aarch64`, `arm64`,
  `x86_64`, `riscv64`, …). Absent `longBit` ⇒ no warning. A mixed kernel/userspace is a valid
  configuration and must never be reported as failed.
- Install must not consult doctor; doctor stays read-only and diagnostic.

### D5 — One implementation point

D2 lives in one `audb-core` module (query constant + parse + compatibility) used by
`system_package.rs:152-155`, `system.rs:233-235`, and referenced by D4's inventory. The shared
implementation owns query construction, parsing **and** the decision — not just a predicate
while callers keep their own queries. Each installer keeps its own transport adapter and its
existing error/cleanup semantics (`system_package.rs:157-178`; APM dispatch/cancellation at
`system.rs:246-248` is not merged).

---

## 4. Verification plan

### 4.1 Unit tests (host-runnable)

`parse_device_arch`: two tokens; one token; unresolved `%{_target_cpu}` echo; empty output;
whitespace/newline noise; tokens with `%`/`{}`/`;`/`/`; duplicate tokens.

`ensure_package_arch` (table-driven):

| observed tokens | package.arch | expected |
| --- | --- | --- |
| `[armv7hl, arm]` | `armv7hl` | accept (t1100, measured) |
| `[armv7hl, arm]` | `noarch` | accept |
| `[armv7hl, arm]` | `arm` | accept (status quo, rpm decides) |
| `[armv7hl]` | `armv6hl` | reject (stricter than rpm, deliberate) |
| `[aarch64]` | `armv7hl` | **reject** (preserves `system_package.rs:398-399`) |
| `[aarch64]` | `aarch64` | accept (no aarch64 regression) |
| `[arm]` | `aarch64` | reject |
| `[]` | `armv7hl` | error |

Shell behaviour of the probe itself, with a fake `rpm` executable on `PATH` (the existing mocks
cannot prove this): resolving rpm, rpm echoing an unresolved macro, rpm absent/failing, and
`rpm` printing extra whitespace.

### 4.2 Build/artifact assertions

`rpm -qp --qf '%{ARCH}'` == `armv7hl`; `rpm -qpl` shows `/usr/lib/qt5/qml/Audb…` and
`/usr/lib/maliit/plugins/…` and `/usr/lib/systemd/system/audb-agent.service`; `readelf -A`
`Tag_ABI_VFP_args` on all four ELFs; `PT_INTERP` `ld-linux-armhf.so.3` on the two executables;
no `-neon` in the target cfg; `readelf --version-info` symbol floors ≤ device glibc 2.38.

### 4.3 On-device (t1100 only)

Register `t1100` (all other registry entries untouched), `--check-only`, then install +
service restart; capture pre/post `rpm -q audb-agent`, `systemctl status audb-agent`,
`audb --device t1100 doctor`, and one non-touch command. Touch/geometry verification belongs
to integration branch U (which carries A's fbdev fallback) and does not block this branch.
`setup-device`'s default `packages/audb-agent.rpm` (`audb-client/src/main.rs:658,891-898`)
stays as-is; the armv7hl file is passed explicitly.

### 4.4 Hardening (small, in-scope)

`cfg`-gated compile-time assertion that `size_of::<InputEvent>() == 16` on 32-bit ARM, so a
future libc `timeval` change cannot silently desync the uinput compat ABI. `UinputDevice`
has no `time_t`/pointer fields and needs none.

---

## 5. Review dispositions

Architect (Sol 6.1) verdict: *conditionally approve the approach, not D2 as written*. Critic
(grok-4.7) verdict: *CHALLENGE*. Both are archived in `docs/reviews/`.

**Accepted (must-fix):**

- **R1 Query the package namespace.** One remote command, both macros, parsed in Rust, strict
  token validation, no shell fallback → §3 D2. ✔
- **R2 No alias, no rewriting `arm` → `armv7hl`; keep `arm` packages out of any new grant;
  never widen to `armv6hl`.** Rule 3 from the v1 draft is **deleted** → §3 D2. ✔
- **R3 Byte-for-byte non-regression.** The union of observed tokens keeps today's `_arch`
  acceptance, so no working device can regress. This is the one place where the design does
  not follow the reviewers' "preferred-order" recipe: preferred-order needs the aarch64
  `%{_target_cpu}` observation (blocked on approval to probe a non-t1100 device) before it is
  safe, and would newly reject `arm`-arch packages the gate accepts today. **Flagged for the
  operator**; if the aarch64 probe confirms `%{_target_cpu} = aarch64`, switching to
  preferred-order is a one-line narrowing and equally safe. ✔ (disclosed deviation)
- **R4 Doctor: keep `architecture` as uname, add optional fields, warn on bitness mismatch
  (never fail), install must not consult doctor** → §3 D4. ✔
- **R5 Shared implementation owns query + parse + decision; both install paths; keep APM and
  rpm `--test` semantics separate** → §3 D5. ✔
- **R6 Float ABI covers the C++ payload, not only Rust; flags on `g++` and the link line;
  prove with `Tag_ABI_VFP_args` on all four ELFs and `PT_INTERP` on executables, not
  `e_flags`** → §3 D1. ✔
- **R7 libdir from the sb2 rpm that runs rpmbuild; `--arch` assertion-only; wipe topdir, glob
  the output, assert header + payload; keep `spec:47` literal; never use `%{_libdir}` as a
  filesystem path** → §3 D3. ✔
- **R8 Do not state SDK macro values as known; probe both targets before editing the spec** →
  §1.3 + §3 D3 (the build fails closed against the device contract). ✔
- **R9 `armv7l` is not a package arch**; do not derive `_target_cpu` from `_host_cpu` → §1.1
  provenance note. ✔
- **R10 Doc corrections** (v1 claims): rule 3 "matches rpm's compat model" (false — deleted);
  `EF_ARM_ABI_FLOAT_HARD` as the required proof (replaced); "rpm handles `armv6hl` at install
  time" (the pre-gate returns first — corrected); SDK armv7 `%{_libdir}` stated as fact
  (demoted to unmeasured); glibc 2.17 / FPU baseline stated as fact (demoted to a check);
  "agreement by construction" (corrected to "fail-closed assertion against the device
  contract"). ✔
- **R11 Shell-level tests with a fake `rpm`, and `--test`/cleanup integration tests retained**
  → §4.1. ✔
- **R12 Linker wrapper should fail when `gcc -dumpmachine` does not match the Cargo triple**
  → §4.2 (implemented as a build-time guard).
- **R13 `setup-device` default RPM path unchanged; pass the 32-bit file explicitly** → §4.3. ✔
- **R14 Reject a build that emits anything other than the single expected binary RPM** → §3 D3. ✔

**Recorded, not adopted now (nice-to-have or unmeasurable today):**

- Unique/serialized rpmbuild topdirs per arch: current per-arch paths
  (`build-agent-rpm.py:34,72`) do not collide across architectures, only across concurrent
  builds of the same arch; noted as a residual risk rather than reworked.
- rpm dependency and GLIBCXX/CXXABI floors: verified after the build, not designed around.
- `armv7hnl`/NEON and other 32-bit ARM devices remain out of scope by design.

---

## 6. Phase 1 baseline findings (host `190bbaa`) — pre-existing, unrelated to this task

| Check | Result |
| --- | --- |
| `cargo test -p audb-protocol` | pass |
| `cargo test -p audb-core` | **42 pass / 1 fail** — `setup::tests::migrates_audb2_wrapper_in_place` (`setup.rs:303`) asserts a `/tmp/` path; macOS tempdirs are `/var/folders/…` |
| `cargo test -p audb-client --test devices` | **2 pass / 16 fail** — see below |
| `cargo clippy -p audb-core -p audb-protocol -p audb-client --all-targets` | clean |
| `cargo test/clippy --workspace` | **impossible on the host** — `audb-agent` is Linux-only (`libc::SO_PEERCRED`, `libc::ucred`, `audb-agent/src/input.rs:27-28`); no `.github` CI and no repo-local test entry point |

**Test-isolation defect (needs an operator decision; outside the brief's root-cause map).**
`audb-client/tests/devices.rs:26-27` isolates each test with `XDG_CONFIG_HOME`/
`XDG_CACHE_HOME`, but the binary resolves state through `directories::BaseDirs`
(`audb-core/src/config.rs:2,55-59`), which on macOS ignores XDG and returns
`~/Library/Application Support`. All 18 integration tests therefore share the operator's real
registry and the suite is **destructive**: the baseline run appended a `phone` device and
rewrote `emulator.qmpSocket`. The registry was restored byte-exactly (1790 B, 4 devices,
`…/T/.tmpucfFmF/delayed.sock`); a pre-restore copy is kept at
`~/Library/Application Support/audb/devices-v1.json.pre-armv7-testrun.bak`. Until this is
resolved, `cargo test` green is defined as: host-buildable crates green on the host, the agent
cross-clippy'd for a Linux target, and Linux-run integration tests. Two options are on the
table: (a) run integration tests on Linux (device or container), or (b) a ~3-line fix so
config discovery honours `XDG_CONFIG_HOME` when set — the intent the tests already encode.
(b) is outside the brief's stated scope and awaits approval.

---

## 7. Residual risks / not yet verified

- SDK container unreachable: the decisive armv7 hard-float sysroot measurement, Qt5/Maliit dev
  availability, and the build itself are pending.
- `aarch64` devices' `%{_target_cpu}` / `%{_libdir}`: unmeasured (no approval to probe
  non-t1100 devices). D2's union makes the gate safe without it; the values are still wanted
  for documentation and for the optional preferred-order narrowing.
- Unsigned RPM: install runs plain `rpm -ivh`/`-Uvh` with no `--nosignature`
  (`system_package.rs:260-266`); if t1100 enforces signatures, install fails and signing keys
  need separate approval.
- Device is Aurora 5.2.0.180 while the SDK target is 5.2.1.200 — same ABI family, unverified
  runtime dependency differences (Qt5/Maliit, GLIBCXX floors).
- uinput `InputEvent` layout across the 32/64-bit boundary is reasoned, not device-verified;
  touch verification belongs to branch U.
- `armv7hnl`/NEON and other 32-bit ARM devices are untested by design.

## 8. Out of scope

Touch/geometry verification on t1100 (branch U, with A's fbdev fallback); signing and
publishing; changes to `final-judge`/`architect`/critic role configuration; any push, merge or
history rewrite.
