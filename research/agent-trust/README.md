# Developer agent installation and IMA

Investigated on 2026-11-06 using Aurora 5.2.2.454 / KVADRA_T (aarch64),
librpmsign-external 0.9.8.1 and ima-certificates-loader 0.3.2.

## Cause

The old `--undefine=__transaction_validation` removed both pre-transaction
validation and post-transaction certificate registration. `external_ima.so`
still wrote file signatures. Consequently successful RPM installation did not
establish the agent's own entry in the signatures database.

The native loader reads certificates referenced by installed packages, excluding
revoked keys. It loads developer certificates into `.ima`; it does not scan an
arbitrary certificate directory. It may return zero after a key-loading failure.
The observed SDK key was referenced only by VoiceAssistant before the fix.

## Implemented route

Default installations retain native RPM validation. For explicit developer-device
installs, `--developer-key-id` pins the full certificate key ID. The wrapper makes
an owner-only temporary configuration and sets `RPMSIGN_EXTERNAL_ROOTFS` for the
RPM process. That variable selects the configuration location; configuration
values for the real database, CA, revocation and developer-mode paths remain
unchanged. Only `system_developer_keyid_path` points at the temporary pin.

The native plugin compares the pin to the verified developer certificate, checks
revocation and registers package/certificate after installation. No direct writes
to the signatures database are performed by audb. Afterward audb invokes the
native loader, restarts its service and checks an actual agent response.

This is an internal mechanism observed in shipped binaries, not a documented
public Aurora installation API. It is deliberately explicit. The system service
is not a Regular-profile application, even when a Regular development certificate
is used. Installation continues to require root/devel-su.

Native sources for signing operations: [Aurora rpmsign-external documentation](https://developer.auroraos.ru/doc/sdk/tools/rpmsign_external).
The process-specific configuration contract was established by REA:

- Configuration environment: `ev_e8d1cfd07ae29787cde640a70ae84b33e0b43aed272a5519dbf26e497eb09d40`.
- Exact signer comparison: `ev_5d5f61addec61ecf2525d9c1d848da02c7871bdcbfe779027c9afaefed377ca2`.
- Database/CA path getters: `ev_2e4f34bcf5b32573b25f97c2770add2318399e4f33262d2a437b6f5f49f96468`,
  `ev_5e86b2c6cf1ca92769f488e6e41226811b28b151c4a3fb6cea55ff4d9792ac1f`.
- Validation pre/post: `ev_fccea53795a531c41b722176bebe25a39e52655a328c670b8dd7fc16f20f0a27`,
  `ev_3ce897fae3bc23cd57a40e7583e783fd689441774724e54f9a6aa19bd18a3080`.

## Verification and limits

- Native `--test` rejects a Regular signer without explicit pin and rejects an
  incorrect pin. The correct full key ID succeeds, with validation enabled.
- A separate `--test` using a private database copy with the SDK certificate
  and every reference to it removed also succeeds. The live database was not
  changed by that probe; this checks that admission does not depend on a prior
  application registering the key.
- A real upgrade to 0.4.2 registers `audb-agent` against its SDK certificate in the
  device's real signatures database. SQLite integrity check on a read-only copy
  passes. VoiceAssistant remains installed but no longer supplies the only package
  reference keeping this certificate active.
- Native loader returns no error output; agent 0.4.2 responds with input,
  screenshot and permission capabilities. Actual taps change VoiceAssistant from
  idle to listening and back; PNG captures are 2000×1200. `doctor` is healthy.
- All 167 workspace tests, Clippy with `-D warnings`, formatting and packaging
  self-tests pass. All three RPM architectures were built and their ELF payloads
  verified. Only aarch64 was exercised on hardware for this release.
- Host tests execute the actual shell wrapper against isolated configuration,
  checking preservation of CA/database/revocation fields, rejection of unfamiliar
  layouts, cleanup and propagation of transaction failure. Installer tests cover
  ordering, check-only, partial failures, architecture and credential handling.
- The device was already running with the SDK certificate in its keyring. No key
  was revoked/removed, no factory reset and no reboot were performed. This proves
  independent persistent registration and working upgrade, not an observed cold
  boot from a factory image. The loader's startup contract supports that expected
  behavior, but fresh-device execution remains to be verified independently.
- The user had disabled APM application validation before this test. audb did not
  alter that setting. RPM signature validation was enabled for these tests.

Local receipts live under ignored `target/release-0.4.2/evidence/`; they are not
release assets and may contain device-specific paths. Signing keys and the live
certificate database are not distributed.
