The arch gate is querying the wrong rpm namespace, and the compatibility rule proposed on top of that is not rpm's model. The packaging and ABI plan is pointed the right way, but it over-claims measurements and misses the C++ objects that actually have a float ABI.

# Independent view

What I would design, from the sources, before treating the proposal as given:

- **Package arch is `%{_target_cpu}`, compared by equality, plus `noarch`.** On this device that value is `armv7hl`, which is also the only arch in the installed census and the cpu in `/etc/rpm/platform`. `%{_arch}` (`arm`) and `%{_host_cpu}` (`armv7l`, as claimed) are different strings and are not the package namespace. One remote command returns both `%{_target_cpu}` and `%{_arch}`; Rust picks the first token only if it matches `^[A-Za-z0-9_]+$`, otherwise the second, otherwise it errors. No alias table. `arm` packages stay rejected on an `armv7hl` machine because rpm's `arch_compat` chain does not list them (`system_package.rs:260-263` has no `--ignorearch`, so rpm is already stricter than an equivalence class). The same function is called at `system_package.rs:152-155` and `system.rs:233-235`.
- **Do not reimplement `arch_compat`.** Exact match is deliberately stricter than rpm (it will refuse `armv6hl`). That is a UX pre-check. The transaction is the real authority.
- **Rust triple `armv7-unknown-linux-gnueabihf`.** The device loader is `/lib/ld-linux-armhf.so.3` and rpm's optflags are `-mfloat-abi=hard -mfpu=vfpv3-d16`. Mer's `armv7l-unknown-linux-gnueabi` string is a vendor tuple, not Rust's soft-float target. `InputEvent` at `audb-agent/src/bin/audb-agent.rs:25-30` embeds `libc::timeval`, which is 8 bytes on this triple and 16 on aarch64; the 32-bit layout is what an aarch64 kernel's compat `input_event` expects. A soft-float triple would still be 32-bit and would still be the wrong loader.
- **Float ABI is a property of every shipped ELF, not of the Rust triple.** `scripts/build-agent-rpm.py:51` and `:61` compile `display` and `libaudbinput.so` with sb2 `g++` and never go through `scripts/aurora-linker.sh:4`. If that gcc is soft-float, the fix is `-mfloat-abi=hard -mfpu=vfpv3-d16 -march=armv7-a` on those commands and on the Rust link line. The build fails unless every executable has `PT_INTERP` `ld-linux-armhf.so.3` and every ELF has `Tag_ABI_VFP_args: VFP registers`. Do not key off `e_flags` bit `0x400`.
- **Libdir comes from the same sb2 rpm that runs rpmbuild, and the build fails closed if it disagrees with the device.** Device contract: `armv7hl` + `/usr/lib` for Qt/Maliit (`/usr/lib/qt5/qml`, `/usr/lib/maliit/plugins`). aarch64 contract, to be confirmed inside that sb2 target before the spec changes: `aarch64` + `/usr/lib64`. Systemd stays `/usr/lib/systemd/system` (`packaging/audb-agent.spec:47`) because that path is not `%{_libdir}`. `--arch` may only assert; it must not override. Find the output RPM by glob after deleting the rpmbuild topdir, then assert `%{ARCH}` and the file list.
- **Doctor stays diagnostic.** Keep `architecture` as `uname -m` (`readiness.rs:14`). Add `rpmArch`, `rpmLibdir`, `longBit`. Add a **warning** check (not a failure: `readiness.rs:352` ignores warnings) when `longBit=32` and `uname -m` is a 64-bit machine name. Install must not consult doctor.

# Critique

## Answers

**1. `%{_target_cpu}` is the right primary source. One command, parsed in Rust, not an in-shell fallback.**

`%{_arch}` is the bug, not the namespace. Both gates compare the RPM `%{ARCH}` to `rpm --eval '%{_arch}'` (`system_package.rs:152-155`, `system.rs:233-235`). On t1100 that is `armv7hl` against `arm`, so every real package is refused. `%{_target_cpu}` is the string that matches the census and `/etc/rpm/platform`.

aarch64 regression is real and unmeasured. The phone path keeps working only if `%{_target_cpu}` evaluates to `aarch64` there. The document says "expected aarch64 elsewhere" (`docs/armv7-support.md:141`). That is a hope. Existing phones prove `%{_arch}` is `aarch64`, because installs work today. They do not prove the two macros are equal. Measure `rpm --eval '%{_target_cpu} %{_arch}'` on one aarch64 device before changing the gate.

One round trip is right. Putting the fallback in the remote shell (`docs/armv7-support.md:144`) is the wrong shape: `system_package.rs:301` and `audb-client/tests/devices.rs:97` only match `rpm --eval` and return one token, so the shell `case` would never run under test. Return both tokens from `rpm --eval '%{_target_cpu} %{_arch}'` and decide in Rust. Preserving the mock by hiding the policy in shell is not testability.

**2. Neither a directional alias nor a symmetric class. Exact match after the query fix.**

Rule 3 (`device_arch == "arm" && package.arch == "armv7hl"`, `docs/armv7-support.md:153-154`) does not fire on t1100 once `device_arch` is `%{_target_cpu}` (`armv7hl`). The row that "proves" t1100 is exact match. The `arm`/`armv7hl` accept row only covers the fallback, which is the broken macro, not the machine.

It is also not rpm's model. rpm's chain is directed and starts at the **install** arch: `armv7hl → armv6hl → noarch`, with no `armv7hl: arm` edge. It does not say "if the family macro says `arm`, accept `armv7hl`". The open-question claim that the directional rule "matches rpm's own compat model" (`docs/armv7-support.md:243`) is false. Section 2 already says this is the wrong namespace, not a missing alias (`docs/armv7-support.md:98-99`). Rule 3 puts the alias back.

A symmetric `{arm, armv7hl}` class is worse: it would accept plain-`arm` packages that this rpm will not install. Excluding plain `arm` on an `armv7hl` machine is the defensible, narrowly proven rule (census 940/940, no compat edge). Do not widen the gate to `armv6hl` either. The comment that rpm handles `armv6hl` "at install time" is wrong while the pre-check returns before `rpm --test`.

**3. Deriving libdir and the RPM arch from the sb2 target's rpm is sound only as a fail-closed assertion, and the design misses three pitfalls.**

Using sb2 `rpm --eval '%{_target_cpu} %{_libdir}'` for the Qt/Maliit staging path is better than a hand map from `--arch`. `build-agent-rpm.py:54` and `:57` hardcode `usr/lib64`, and the spec repeats it at `packaging/audb-agent.spec:45-46`. Those must move together, and only those. `/usr/lib/systemd/system` must stay literal.

Pitfalls the text does not close:

- The sentence "`%{_libdir}` is `/usr/lib` on the armv7 target and `/usr/lib64` on the aarch64 target" (`docs/armv7-support.md:175-176`) states the **SDK** macro as fact. Section 1.3 item 4 says the armv7 target's `%{_libdir}` is unmeasured. The device's `/usr/lib` is measured. Those are different rpm installations. If both the query and `%files` expand to `/usr/lib64` inside a mis-selected target, they "agree by construction" and still install where Maliit does not look. `--arch` as an "override/assertion" (`docs/armv7-support.md:186`) reopens the drift if override wins.
- rpmbuild is not passed `--target` (`build-agent-rpm.py:76`). The output directory is whatever that rpm puts in `RPMS/<arch>/`. This device exists to show `%{_arch}` and `%{_target_cpu}` diverge, so predicting `RPMS/%{_target_cpu}/` can look at the wrong directory. The topdir is only `mkdir -p`'d (`build-agent-rpm.py:72-73`), so a stale RPM at the predicted path can be copied. Glob `RPMS/*/*.rpm` after wiping the topdir, then assert `%{ARCH}` and `rpm -qpl`.
- `%{_libdir}` is an install path inside the payload (`usr/lib` vs `usr/lib64` in the tarball). It must not be used as a host or sb2 filesystem path. The script already avoids rpmbuild on shared host paths (`build-agent-rpm.py:70-71`).

**4. `armv7-unknown-linux-gnueabihf` is the correct triple. If the SDK gcc is soft-float, flags go on every gcc/g++ invocation, and the proof is the ELF attribute, not `e_flags`.**

The Mer host string `armv7l-unknown-linux-gnueabi` must not be passed to rustc. Soft-float `armv7-unknown-linux-gnueabi` would ask for `/lib/ld-linux.so.3`. The device's hard-float loader and `vfpv3-d16` optflags decide this. I could not re-check the "no `ld-linux.so.3`" half; the brief only establishes that `ld-linux-armhf.so.3` is present.

If `gcc -dumpmachine` is aarch64, the sb2 `-t` is wrong and flags will not fix it (the earlier probe already fell back to `aarch64-meego-linux-gnu`). If dumpmachine is arm and `__SOFTFP__` is set, add `-march=armv7-a -mfloat-abi=hard -mfpu=vfpv3-d16` to the linker wrapper **and** to the two `g++` lines. `RUSTFLAGS` alone do not recompile `libaudbinput.so`, which is the file Maliit dlopens. Do not retarget Rust at soft-float to match a soft gcc.

`EF_ARM_ABI_FLOAT_HARD` (`0x400`, `docs/armv7-support.md:131`) is not a reliable rustc/LLVM `e_flags` bit. Require `Tag_ABI_VFP_args: VFP registers` on all four ELFs (`audb-agent`, `audb-agentctl`, `display`, `libaudbinput.so`). Require `PT_INTERP` `ld-linux-armhf.so.3` only on executables; the plugin is a shared object and often has no interpreter. Run `readelf` inside sb2. The device has no `readelf`, and the Mac host should not be assumed to either. `strip = true` in the workspace release profile does not remove `.ARM.attributes` or `PT_INTERP`; still run the check on the staged, post-strip files.

The Rust triple's FPU baseline versus `vfpv3-d16` and `-neon` is **not measured**. The target is not installed. Distro optflags explicitly are not NEON (`armv7hnl` would be). Before shipping, `rustc --print cfg --target armv7-unknown-linux-gnueabihf` must not enable `neon`. If it does, `-C target-feature=-neon` is the minimal extra flag. "glibc 2.17 baseline" is likewise unverified for rustc 1.97.1; `readelf --version-info` after the build is the check, not a fact yet.

**5. The shared gate is necessary and the design already points it at both install paths. Additive doctor fields are not sufficient.**

D5 correctly names `system.rs` and `system_package.rs`. Doctor does not install anything, and nothing in the client gates setup on `healthy`. Fields inside the always-passed `identity` check (`readiness.rs:320`) leave `architecture: aarch64` as the headline. Add a warning check when `longBit` is `32` and `uname -m` is `aarch64` (or `x86_64`). That does not flip `healthy`. Missing `longBit` must not warn, so `audb-client/tests/devices.rs:1025` stays valid.

The gate still does not inspect ELF contents. A package tagged `armv7hl` with a soft-float or aarch64 payload passes it. The build-time `readelf` check is the other half; do not pretend doctor closes that.

## Blockers

- Rule 3 must not ship. It is dead on the measured device, it is not `arch_compat`, and the test that accepts `arm`+`armv7hl` locks the wrong behavior. Policy is `noarch` or exact `%{_target_cpu}`.
- The soft-float fix, as written, never reaches `build-agent-rpm.py:51` and `:61`. A hard-float Rust agent next to a soft-float `libaudbinput.so` is a broken package that the arch gate will accept.
- Do not treat SDK `%{_libdir}` / `%{_target_cpu}` as already known, and do not let `--arch` override them. The build's contract is equality with the device (`armv7hl`, `/usr/lib`) or failure.

## Required changes

- One `rpm --eval '%{_target_cpu} %{_arch}'`, both tokens parsed in Rust, strict `^[A-Za-z0-9_]+$`. Empty or `%` in the first token falls through to exact `%{_arch}` only. Never rewrite `arm` into `armv7hl`.
- Confirm `%{_target_cpu}` and `%{_libdir}` on an aarch64 phone (gate) and inside **both** sb2 targets (packaging) before editing the spec. aarch64 must stay `aarch64` / `/usr/lib64` or the `%{_libdir}` spec edit is an aarch64 regression.
- Wipe the rpmbuild topdir, glob the single RPM, assert `%{ARCH}` and that Qt/Maliit paths are under `/usr/lib` for this artifact and still under `/usr/lib64` for the aarch64 artifact. Leave `spec:47` hardcoded.
- ELF check as in answer 4, including `-neon` if the target spec has it. Apply float flags to g++ and the linker, not only `RUSTFLAGS`.
- Doctor warning described in answer 5. Keep new inventory fields optional.

## Suggestions

- `const` assert `size_of::<InputEvent>() == 16` on the armhf target so a future 64-bit `timeval` in `libc` cannot silently desync the uinput compat ABI (`audb-agent/src/bin/audb-agent.rs:25-30`). `UinputDevice` (`:32-40`) has no pointer or `time_t` fields; leave it.
- `setup-device` defaults to `packages/audb-agent.rpm` beside the binary (`audb-client/src/main.rs:658`, `:892-898`). Pass the armv7hl file explicitly. Do not replace that default with the 32-bit RPM.
- Fail the linker wrapper if `gcc -dumpmachine` does not match the Cargo triple (armhf vs aarch64). The wrapper's default target is still the aarch64 sb2 target (`aurora-linker.sh:4`).
- Reject an rpmbuild that emits any RPM other than the one binary package. The debuginfo defines at `build-agent-rpm.py:76` are unproven on the armv7 rpm.

## Assumptions and unanswered questions

- No shell in this session, so t1100 was not re-queried. Brief facts used as given: `uname -m=aarch64`, `LONG_BIT=32`, `%{_arch}=arm`, `%{_target_cpu}=armv7hl`, `%{_libdir}=/usr/lib`, census `940 × armv7hl` and `211 × noarch`, `ld-linux-armhf.so.3`, optflags `vfpv3-d16`, compat `armv7hl → armv6hl → noarch` with no `arm` edge.
- `%{_host_cpu}=armv7l` and "no `ld-linux.so.3`" are the document's claims, not the brief, and were not rechecked.
- `/usr/lib/rpm/macros:1026` (`docs/armv7-support.md:49-50`) cannot both be `%_target_cpu %{_host_cpu}` and expand to `armv7hl` if `%{_host_cpu}` is `armv7l` (`:31`). Something else sets `%{_target_cpu}` (platform macros or `buildarchtranslate`). Do not "simplify" the query to `%{_host_cpu}`: `armv7l` is not an installed package arch.
- Assumed, not measured: aarch64 `%{_target_cpu}` and `%{_libdir}`; the armv7 sb2 target's existence, gcc float default, Qt5/Maliit dev packages, and rpmbuild output directory; rustc 1.97's gnueabihf feature set and glibc symbol floor; whether the T1100 CPU has NEON (only the distro optflags were measured).
- APM (`system.rs:233`) has no `--test` backstop. A too-lax gate there is not saved by rpm. Another reason not to accept `arm` or `aarch64` by alias.
- Transitive crate graph was not fully walked. Direct `audb-agent` deps in `Cargo.lock` match the pure-Rust claim; no repo `build.rs`.

## Factual status

No fatal flaw in the goal. The device is a 32-bit hard-float ARM userspace, and the current gate cannot install its packages.

Wrong or unverifiable as written:

- Rule 3 "matches rpm's compat model" (`docs/armv7-support.md:243`).
- macros:1026 "resolves to `armv7hl`" contradicts the same doc's `%{_host_cpu}=armv7l` (`:31` vs `:49-50`).
- SDK armv7 `%{_libdir}` stated as known (`:175-176`) while §1.3 lists it unmeasured.
- `EF_ARM_ABI_FLOAT_HARD` as a required proof (`:131`).
- "rpm handles `armv6hl` at install time" while the gate returns first.
- glibc 2.17 / rustc FPU baseline, stated ahead of a binary that does not exist yet.

## Measure this before production packaging code

One command inside the intended sb2 target, not the aarch64 fallback:

`gcc -dumpmachine`, the predefined macros `__ARM_PCS_VFP` / `__SOFTFP__` / `__ARM_FP`, and `rpm --eval '%{_target_cpu} %{_arch} %{_libdir}'`, plus `pkg-config` for the Qt5 modules the script already links.

Pass only if dumpmachine is arm, VFP PCS is hard, `target_cpu=armv7hl`, and `libdir=/usr/lib`. The earlier `aarch64-meego-linux-gnu` probe is a failed measurement, not evidence the target exists. Until that passes, do not treat float flags or `%{_libdir}` as settled. The arch-gate edit can be written before docker is back, but only after one aarch64 device confirms `%{_target_cpu}` is `aarch64`.

# Verdict

`CHALLENGE`