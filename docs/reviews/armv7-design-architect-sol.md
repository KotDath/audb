# Architectural review: armv7 support

**Verdict: conditionally approve the approach, not D2 as written.** No fatal flaw prevents T1100 support. The unconditional `arm → armv7hl` fallback is unjustified, and packaging needs stronger target/output validation before implementation proceeds.

Read the complete design and all six cited source files. Existing device measurements are accepted as supplied; no Docker operations, device changes, or file writes were performed.

## 1. Context and constraints verified in source

- Both installation paths independently query `%{_arch}` and compare against package metadata: `audb-core/src/system_package.rs:152-155`, `audb-core/src/system.rs:233-235`.
- System installation already performs RPM `--test` before the real transaction. Preserve its error, cleanup, and verification behavior (`system_package.rs:157-178`). APM dispatch has different cancellation/retention semantics; do not merge these installers (`system.rs:246-248`).
- Packaging genuinely hardcodes `lib64` in staging and `%files` (`scripts/build-agent-rpm.py:54-57`, `packaging/audb-agent.spec:45-46`).
- Rust is not the whole artifact: packaging separately compiles a Qt executable and a shared QML plugin using `g++` (`build-agent-rpm.py:51-65`).
- The linker already accepts an SDK target through the environment (`scripts/aurora-linker.sh:3-4`).
- Doctor’s inventory parser accepts arbitrary additive fields, but identity explicitly selects fields; adding inventory entries alone does not expose them there (`audb-core/src/readiness.rs:136-155,320`).

## 2. Recommendations on the five decisions

### D2 namespace: use `_target_cpu`, with validated fallback

**Yes:** `_target_cpu` is the appropriate primary package-namespace observation for this deployment. Keeping `_arch` alone perpetuates the measured T1100 failure.

Use **one remote shell invocation**, potentially executing two local RPM evaluations. This reduces round trips and gives both installers the same observation protocol. Fall back for an empty/unexpanded macro, not indiscriminately for an RPM execution failure. Reject an invalid fallback value.

Aarch64 has no inherent regression if both macros resolve to `aarch64`. Nevertheless, “expected aarch64 elsewhere” is an assumption, not verified evidence (`docs/armv7-support.md:141`). Test primary success, fallback success, unresolved values, command failure, and whitespace normalization.

Existing mocks merely matching `command.contains("rpm --eval")` cannot prove that fallback logic works (`system_package.rs:301`). Add shell-behavior tests with a fake RPM executable; retain installer integration tests.

### D2 compatibility: directional is better than symmetric—but remove rule 3

RPM compatibility is directional. A symmetric `{arm, armv7hl}` equivalence class would incorrectly authorize plain-arm packages on this measured armv7hl system. **Keep excluding those packages:** the supplied RPM compatibility chain provides no such permission.

However, **generic `device_arch == "arm"` does not prove armv7 hard-float**. The measured device returns `armv7hl` from the new primary query, so it needs no exception. Evidence about this one device does not justify all machines whose fallback says `arm`.

Recommended rule:

> Accept `noarch` or exact equality of validated package/device RPM architecture. Otherwise reject.

If legacy normalization later becomes necessary, require independent platform evidence and normalize the **device observation** before comparison. Do not declare `arm` a hard-float spelling.

The proposed family registry and “eq-class/from/to” encoding (`docs/armv7-support.md:162-166`) adds machinery without establishing that invariant. Explicit equality plus negative tests is clearer and safer. This pre-gate remains deliberately narrower than RPM, not an implementation of RPM’s compatibility graph.

### D3 libdir: sound source, but not agreement “by construction”

**Derive staging libdir from the selected sb2 target**, and use `%{_libdir}` in `%files`. Do not maintain an architecture-to-libdir table.

Necessary qualifications:

- Probe before compiling/staging. Validate architecture and an absolute, normalized libdir. When joining with `stage`, remove the leading `/`; otherwise Python’s path join escapes the staging tree.
- Make `--arch` **assertion-only**, absent by default. “Override/assertion” is ambiguous (`docs/armv7-support.md:183`); an override reintroduces competing truth.
- Separate SDK target name, Rust triple, RPM architecture, and libdir. Cross-check their compatibility; RPM metadata alone cannot detect an aarch64 Rust payload mislabeled armv7hl.
- RPM output directory/name follow rpmbuild’s effective configuration, including `_rpmfilename` and possible spec overrides—not universally `_target_cpu`. Probe the effective rpmbuild context or discover the produced main RPM in a clean output directory, then verify its header.
- Preserve native-engine rpmbuild storage and disabled debug packaging (`build-agent-rpm.py:70-77`). Moving rpmbuild to shared staging paths risks the already documented sb2 postprocessor mapping problem.
- `%files` directory ownership recursively includes the QML module; macro substitution introduces no special globbing problem. But rpmbuild postprocessing can still alter packaged ELF files. Verify the **packaged payload**, not only pre-package staging.

The present spec has no `BuildArch` or `_libdir` override, so the proposed values should agree (`audb-agent.spec:1-25,40-49`). They still require an artifact assertion. Target RPM libdir also does not alone prove Qt/Maliit search paths; confirm the armv7 SDK’s Qt configuration.

### D1 ABI: correct Rust triple; flags must cover every native artifact

**Use `armv7-unknown-linux-gnueabihf`.** The hard-float loader and RPM optflags outweigh the historical Mer `gnueabi` naming. Do not switch Rust to soft-float to match that string.

If GCC defaults soft-float **but the selected sysroot has usable hard-float libraries**, the minimal fix is:

- GCC linker-driver arguments: `-mfloat-abi=hard -mfpu=vfpv3-d16`;
- the same ABI/FPU arguments on both native `g++` builds;
- `-march=armv7-a` for native compilation unless already guaranteed.

For Rust, GCC arguments must be passed as linker arguments, for example `-C link-arg=...`, not bare GCC flags in `RUSTFLAGS`. No Rust target change is needed. A soft-float-only sysroot cannot be repaired with flags; select/provision the correct SDK target instead.

Require ELF32/ARM/EABI5, `EF_ARM_ABI_FLOAT_HARD`, and appropriate VFP argument attributes on both Rust executables, the Qt helper, and the QML shared library. Executables must request `/lib/ld-linux-armhf.so.3`. Check runtime symbol requirements, including GLIBCXX/CXXABI for C++, against device libraries.

Rust’s glibc baseline is not the built artifact’s maximum requirement; SDK linkage can raise it (`docs/armv7-support.md:124-125`).

### D4/D5: additive inventory is sufficient; shared behavior must include acquisition

**Yes:** optional `rpmArch`, `rpmLibdir`, and `longBit` plus one shared architecture implementation are sufficient for this scope.

- Reuse the same namespace/fallback logic for doctor and both installation paths.
- Missing optional tools or unresolved macros must yield unavailable fields, not fail the SSH inventory.
- Add fields explicitly to identity (`readiness.rs:320`).
- Do not mark an aarch64 kernel with 32-bit userspace unhealthy: this is a valid supported configuration. An explanatory identity note is useful; a failure check is wrong.
- The shared implementation must own query construction, parsing, and compatibility—not just a boolean predicate while callers duplicate queries.
- Both system installation and APM must reject incompatibility before their respective privileged transaction/dispatch, preserving cleanup semantics.

## 3. Concrete interface and module ownership

Use a small private `rpm_arch` module, or extend the existing shared RPM helpers in `system_package.rs`; `system.rs:178` already imports those helpers. No new transport abstraction is necessary.

Suggested interface:

```text
PACKAGE_ARCH_QUERY: shell command
parse_device_arch(stdout) -> CoreResult<DeviceRpmArch>
ensure_package_arch(device_arch, package_arch) -> CoreResult<()>
```

Contract:

- Query prints one resolved architecture.
- Parser trims and rejects empty, unresolved, or malformed output.
- Gate accepts only exact equality or `noarch`.
- Query errors propagate; mismatch preserves the existing invalid-package error category.
- No cached architecture or persistent migration state.

Each installer retains its existing transport adapter. Doctor embeds the same query logic in its inventory command and treats acquisition failure as optional diagnostic absence.

## 4. Rejected simpler alternatives

- **Keep `_arch` and add an alias:** hides the namespace error and grants permissions without hard-float evidence.
- **Symmetric equivalence:** contradicts the measured RPM compatibility model.
- **Hardcode armv7 → `/usr/lib`:** duplicates target configuration and cannot detect target-selection mistakes.
- **Let RPM reject everything:** simpler, but loses the existing early rejection before privileged execution/APM dispatch. Preserve that behavior rather than silently changing it.

## 5. Must-fix items and risks

### Implementation blockers

1. Remove unconditional rule 3 and the assertion that generic `arm` denotes hard-float.
2. Measure a genuine, usable armv7hl SDK target; do not accept sb2 fallback output.
3. Define `--arch` as assertion-only and validate effective rpmbuild metadata, Rust payload architecture, and packaged paths.
4. Apply ABI checks to **all** ELF artifacts; cover native compilation if flags are required.
5. Share namespace acquisition/parsing across doctor and both gates; test fallback behavior and pre-dispatch rejection.

### Suggestions / residual risks

- Use unique build topdirs or serialize builds. Current per-architecture staging/topdirs collide under concurrent builds (`build-agent-rpm.py:34-37,72-77`).
- Check generated RPM dependencies against the older device runtime before deployment.
- Keep signature-policy uncertainty explicit.
- Same NVR for the new architecture is reasonable; additive doctor fields need no report-version migration.

## 6. Incorrect/unverified claims and questions the judge must retain

- **Unsupported:** rule 3 is “narrowly measured,” and `arm` has an asserted hard-float identity (`docs/armv7-support.md:154-166`).
- **Internally inconsistent explanation:** `_host_cpu` is reported as `armv7l`, yet the cited `_target_cpu %{_host_cpu}` definition supposedly resolves to `armv7hl` (`:31,49-50`). An override may explain this; document effective values without inventing macro provenance.
- **Misleading:** RPM “handles” excluded armv6hl packages at install time (`:168-170`); the proposed pre-gate prevents them reaching RPM.
- **Unverified:** armv7 SDK existence, defaults, libdir, Qt availability, output layout, and runtime compatibility. The document mostly labels this correctly.
- **Overstated:** staging/spec agreement “by construction” and glibc compatibility inferred from Rust’s baseline (`:124-125,181-182`).

**Single most important measurement:** establish that the explicitly selected sb2 armv7hl target actually provides a usable **hard-float compiler/sysroot**, proven by a small linked ELF—not merely its target name or `gcc -dumpmachine`. That determines whether the proposed build path is real before production code is written.