#!/usr/bin/env python3
"""Cross-build audb-agent and package it using the Aurora build-tools container.
Signing is a separate step. Requires the installed Rust target and a running container.

Everything architecture-specific is derived from the *container's own rpm* asked for the
selected platform (`rpm --target <platform> --eval '%{_target_cpu} %{_arch} %{_libdir}'`) and
asserted against the device contract below, so a mistyped platform/compiler fails the build
instead of producing a plausible-looking RPM for the wrong platform.

`--self-test` checks that logic without Docker.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[1]

# Aurora's own armv7hl optflags (rpmrc): Armv7-A, hard-float ABI, VFPv3-D16 (not NEON).
# The cross gcc defaults to NEON, which is *not* the Aurora baseline for 32-bit devices.
# It does not select FPU codegen for the Rust objects: those are already non-NEON per the
# target spec (`rustc --print cfg` reports no feature="neon"); these are link arguments so
# the driver picks the matching sysroot and emits the hard-float ABI attribute.
ARM_FLOAT_FLAGS = ['-march=armv7-a', '-mfloat-abi=hard', '-mfpu=vfpv3-d16']
ARMHF_INTERPRETER = 'ld-linux-armhf.so.3'
# Measured against the aurora-os-build-engine container: aarch64 is native, armv7hl and
# x86_64 are cross-compiled from the toolchain installed by the cross-*-gcc packages.
# `libdir` is the device contract for that rpm architecture: where Qt QML modules and Maliit
# plugins are actually resolved. /usr/lib/systemd/system is deliberately not part of it.
PLATFORMS = {
    'aarch64': {'rustTarget': 'aarch64-unknown-linux-gnu', 'cc': 'gcc', 'cxx': 'g++',
                'sysroot': '', 'rpmTarget': '', 'libdir': '/usr/lib64'},
    'armv7hl': {'rustTarget': 'armv7-unknown-linux-gnueabihf',
                'cc': '/opt/cross/bin/armv7hl-meego-linux-gnueabi-gcc',
                'cxx': '/opt/cross/bin/armv7hl-meego-linux-gnueabi-g++',
                'sysroot': '/opt/cross/armv7hl-meego-linux-gnueabi/sys-root',
                'rpmTarget': 'armv7hl-meego-linux', 'libdir': '/usr/lib'},
    'x86_64': {'rustTarget': 'x86_64-unknown-linux-gnu',
               'cc': '/opt/cross/bin/x86_64-meego-linux-gnu-gcc',
               'cxx': '/opt/cross/bin/x86_64-meego-linux-gnu-g++',
               'sysroot': '/opt/cross/x86_64-meego-linux-gnu/sys-root',
               'rpmTarget': 'x86_64-meego-linux', 'libdir': '/usr/lib64'},
}
# Expected `readelf -h` class and machine substring per rpm architecture.
ELF_EXPECT = {
    'aarch64': ('ELF64', 'AArch64'),
    'armv7hl': ('ELF32', 'ARM'),
    'x86_64': ('ELF64', 'X86-64'),
}


def expected_machine(arch):
    """The `gcc -dumpmachine` prefix that must produce this rpm architecture."""
    return {'aarch64': 'aarch64', 'armv7hl': 'armv7', 'x86_64': 'x86_64'}[arch]


def parse_target_platform(output):
    """Parses `rpm --eval '%{_target_cpu} %{_arch} %{_libdir}'` for the selected platform.

    rpm echoes macros it cannot resolve verbatim, so an unresolved field must never be
    mistaken for a value.
    """
    fields = output.split()
    if len(fields) != 3 or any('%' in f or '{' in f or not f for f in fields):
        raise SystemExit(f'Unresolved rpm platform probe: {output!r}')
    arch, libdir = fields[0], fields[2]
    contract = PLATFORMS.get(arch, {}).get('libdir')
    if contract is None:
        raise SystemExit(f'No device libdir contract for rpm architecture {arch!r}')
    if libdir != contract:
        raise SystemExit(
            f'rpm reports {arch} with libdir {libdir}, but the device contract for {arch} '
            f'is {contract}; refusing to build for the wrong platform'
        )
    return arch, libdir


def check_rust_target(arch, rust_target):
    """A soft-float or mismatched Rust triple would link the wrong loader."""
    machine = rust_target.split('-', 1)[0]
    if machine != expected_machine(arch):
        raise SystemExit(
            f'--rust-target {rust_target} does not produce {arch} '
            f'(expected a {expected_machine(arch)}-* triple)'
        )
    if arch == 'armv7hl' and not rust_target.endswith('gnueabihf'):
        raise SystemExit(
            f'--rust-target {rust_target} is not a hard-float triple; the device runs '
            f'/{ARMHF_INTERPRETER}'
        )


def pkg_config_flags(sysroot, modules):
    """argv that resolves Qt flags for `sysroot`, with the environment applied to
    pkg-config itself.

    Writing them as command-local assignments on the compiler would not work: the shell
    expands `$(pkg-config ...)` *before* those assignments take effect, so pkg-config would
    read the ambient container configuration and could select native headers and libraries.
    Passing the resolved flags as argv also makes a failed pkg-config fatal.
    """
    argv = []
    if sysroot:
        argv += ['env', f'PKG_CONFIG_SYSROOT_DIR={sysroot}',
                 f'PKG_CONFIG_LIBDIR={sysroot}/usr/lib/pkgconfig']
    return argv + ['pkg-config', '--cflags', '--libs', *modules]


def install_paths(libdir):
    """Qt/Maliit install paths inside the payload root, relative to the staging dir.

    Returns (qml_module_dir, maliit_plugin_file) as relative paths without a leading '/'.
    """
    libdir = libdir.lstrip('/')
    return (
        Path(libdir) / 'qt5/qml/Audb/Input',
        Path(libdir) / 'maliit/plugins/zz-audb-input.qml',
    )


def vfp_args_ok(attributes):
    return 'Tag_ABI_VFP_args: VFP registers' in attributes


def interpreter_ok(program_headers, interpreter=ARMHF_INTERPRETER):
    return interpreter in program_headers


def self_test():
    """Exercises every decision that does not need Docker."""
    assert parse_target_platform('armv7hl arm /usr/lib\n') == ('armv7hl', '/usr/lib')
    assert parse_target_platform('aarch64 aarch64 /usr/lib64') == ('aarch64', '/usr/lib64')
    for bad in ['%{_target_cpu} %{_arch} %{_libdir}', 'armv7hl /usr/lib', 'armv7hl arm /usr/lib/x']:
        try:
            parse_target_platform(bad)
        except SystemExit:
            pass
        else:
            raise AssertionError(f'accepted unusable probe output: {bad!r}')

    check_rust_target('armv7hl', 'armv7-unknown-linux-gnueabihf')
    check_rust_target('aarch64', 'aarch64-unknown-linux-gnu')
    for arch, rust_target in [
        ('armv7hl', 'armv7-unknown-linux-gnueabi'),  # soft-float
        ('armv7hl', 'aarch64-unknown-linux-gnu'),  # wrong machine
        ('aarch64', 'armv7-unknown-linux-gnueabihf'),  # wrong machine
    ]:
        try:
            check_rust_target(arch, rust_target)
        except SystemExit:
            pass
        else:
            raise AssertionError(f'accepted {rust_target} for {arch}')

    assert install_paths('/usr/lib') == (
        Path('usr/lib/qt5/qml/Audb/Input'),
        Path('usr/lib/maliit/plugins/zz-audb-input.qml'),
    )
    assert install_paths('/usr/lib64')[0] == Path('usr/lib64/qt5/qml/Audb/Input')
    assert vfp_args_ok('  Tag_ABI_VFP_args: VFP registers\n')
    assert not vfp_args_ok('Attribute Section: aeabi\n')
    assert interpreter_ok('[Requesting program interpreter: /lib/ld-linux-armhf.so.3]')
    assert not interpreter_ok('[Requesting program interpreter: /lib/ld-linux.so.3]')
    # The presets encode measured container paths; a re-imaged container must not silently
    # change which compiler, sysroot or libdir a platform build uses.
    assert PLATFORMS['armv7hl']['rpmTarget'] == 'armv7hl-meego-linux'
    assert PLATFORMS['armv7hl']['cc'].endswith('armv7hl-meego-linux-gnueabi-gcc')
    assert PLATFORMS['armv7hl']['sysroot'].endswith('armv7hl-meego-linux-gnueabi/sys-root')
    assert PLATFORMS['armv7hl']['libdir'] == '/usr/lib'
    assert PLATFORMS['x86_64']['libdir'] == '/usr/lib64'
    assert PLATFORMS['x86_64']['rustTarget'] == 'x86_64-unknown-linux-gnu'
    assert PLATFORMS['aarch64']['rustTarget'] == 'aarch64-unknown-linux-gnu'
    assert parse_target_platform('x86_64 x86_64 /usr/lib64') == ('x86_64', '/usr/lib64')
    # The sysroot must be applied to pkg-config, not to the compiler's command prefix.
    scoped = pkg_config_flags('/sys/root', ['Qt5Gui'])
    assert scoped[0] == 'env' and scoped[1] == 'PKG_CONFIG_SYSROOT_DIR=/sys/root'
    assert scoped[2] == 'PKG_CONFIG_LIBDIR=/sys/root/usr/lib/pkgconfig'
    assert scoped[3] == 'pkg-config' and scoped[4:] == ['--cflags', '--libs', 'Qt5Gui']
    assert pkg_config_flags('', ['Qt5Core']) == ['pkg-config', '--cflags', '--libs', 'Qt5Core']
    print('self-test: ok')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--container', default=os.environ.get('AUDB_SDK_CONTAINER', 'aurora-os-build-engine-5.2.1.200-mb2_kotdath'))
    p.add_argument('--target', default='AuroraOS-5.2.1.200-aarch64', help='sb2 target name passed to the sb2 wrapper (the container shim ignores it)')
    p.add_argument('--platform', choices=sorted(PLATFORMS), default='aarch64', help='target platform preset')
    p.add_argument('--rust-target', default=None, help='Rust triple (default: from --platform)')
    p.add_argument('--cc', default=None, help='C compiler driver inside the container (default: from --platform)')
    p.add_argument('--cxx', default=None, help='C++ compiler driver inside the container (default: from --platform)')
    p.add_argument('--sysroot', default=None, help='target sysroot for pkg-config (default: from --platform)')
    p.add_argument('--rpm-target', default=None, help='rpm/rpmbuild --target (default: from --platform)')
    p.add_argument('--arch', default=None, help='Assertion only: expected RPM architecture. Never selects paths.')
    p.add_argument('--self-test', action='store_true', help='Check the architecture logic without Docker')
    a = p.parse_args()
    if a.self_test:
        return self_test()

    preset = PLATFORMS[a.platform]
    a.rust_target = a.rust_target or preset['rustTarget']
    a.cc = a.cc or preset['cc']
    a.cxx = a.cxx or preset['cxx']
    a.sysroot = a.sysroot if a.sysroot is not None else preset['sysroot']
    a.rpm_target = a.rpm_target if a.rpm_target is not None else preset['rpmTarget']
    rpm_target_args = ['--target', a.rpm_target] if a.rpm_target else []

    def execute(argv, capture_output=False, **kw):
        command = shlex.join(map(str, argv))
        print(command, flush=True)
        result = subprocess.run(list(map(str, argv)), cwd=ROOT, text=True,
                                stdout=subprocess.PIPE if capture_output else None, **kw)
        if result.returncode != 0:
            detail = ((result.stderr or '').strip() if capture_output else '')
            raise SystemExit(f'command failed ({result.returncode}): {command}' + (f'\n{detail}' if detail else ''))
        return result.stdout if capture_output else result

    def capture(argv, **kw):
        return execute(argv, capture_output=True, **kw)

    def run(argv, **kw):
        return execute(argv, **kw)

    def sdk(argv):
        return run(['docker', 'exec', '--user', 'mersdk', a.container, 'sb2', '-t', a.target, '-m', 'sdk-build', *argv])

    def sdk_output(argv):
        return capture(['docker', 'exec', '--user', 'mersdk', a.container, 'sb2', '-t', a.target, '-m', 'sdk-build', *argv])

    # 1. Ask the container's own rpm what the selected platform builds for, and fail closed on
    #    any disagreement with the device contract. `%{_libdir}` is an install path inside the
    #    payload and is never used as a host or container filesystem path.
    arch, libdir = parse_target_platform(
        sdk_output(['rpm', *rpm_target_args, '--eval', '%{_target_cpu} %{_arch} %{_libdir}'])
    )
    check_rust_target(arch, a.rust_target)
    if a.arch is not None and a.arch != arch:
        raise SystemExit(f'--arch {a.arch} contradicts the selected platform, which builds {arch}')
    # Both compiler drivers produce shipped ELFs, so both are checked against the platform.
    machine = ''
    for driver in dict.fromkeys([a.cc, a.cxx]):
        reported = sdk_output([driver, '-dumpmachine']).strip()
        if not reported.startswith(expected_machine(arch)):
            raise SystemExit(
                f'{driver} reports machine {reported}, expected {expected_machine(arch)}-*; '
                f'wrong --platform or missing cross toolchain'
            )
        machine = machine or reported
    qml_rel, plugin_rel = install_paths(libdir)
    float_flags = ARM_FLOAT_FLAGS if arch == 'armv7hl' else []
    # Resolve Qt flags with the sysroot applied to pkg-config (see pkg_config_flags).
    qt_ui_flags = shlex.split(sdk_output(pkg_config_flags(a.sysroot, ['Qt5Gui', 'Qt5Core'])))
    qt_plugin_flags = shlex.split(
        sdk_output(pkg_config_flags(a.sysroot, ['Qt5Quick', 'Qt5Qml', 'Qt5Network'])))
    print(f'building {arch} with libdir {libdir} via {machine} (cc {a.cc})', flush=True)

    env = os.environ.copy()
    env['AUDB_SDK_CONTAINER'] = a.container
    env['AUDB_SDK_TARGET'] = a.target
    env['RUSTC'] = subprocess.check_output(['rustup', 'which', '--toolchain', 'stable', 'rustc'], text=True).strip()
    env['CARGO_TARGET_' + a.rust_target.upper().replace('-', '_') + '_LINKER'] = str(ROOT/'scripts/aurora-linker.sh')
    # The linker wrapper re-checks the machine and uses the platform's compiler driver, so a
    # Cargo triple aimed at the wrong platform cannot silently link against another libc.
    env['AUDB_EXPECT_MACHINE'] = expected_machine(arch)
    env['AUDB_CROSS_CC'] = a.cc
    if float_flags:
        # Link arguments, not bare RUSTFLAGS: the compiler driver is what selects the
        # hard-float sysroot and emits the ABI attribute.
        env['CARGO_TARGET_' + a.rust_target.upper().replace('-', '_') + '_RUSTFLAGS'] = ' '.join(
            '-C link-arg=' + flag for flag in float_flags
        )
    run(['rustup', 'run', 'stable', 'cargo', 'build', '-p', 'audb-agent', '--release', '--target', a.rust_target], env=env)

    work = ROOT/'target'/'agent-rpm'/arch
    stage = work/'stage'
    if stage.exists():
        shutil.rmtree(stage)
    for binary, destination in [('audb-agent', 'usr/sbin/audb-agent'), ('audb-agentctl', 'usr/bin/audb-agentctl')]:
        dest = stage/destination
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT/'target'/a.rust_target/'release'/binary, dest)
    license_file = stage/'usr/share/licenses/audb-agent/LICENSE'
    license_file.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT/'LICENSE', license_file)
    service = stage/'usr/lib/systemd/system/audb-agent.service'
    service.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT/'packaging/audb-agent.service', service)
    display = stage/'usr/libexec/audb-agent/display'
    display.parent.mkdir(parents=True, exist_ok=True)
    display_flags = [a.cxx, '-std=c++11', '-O2', '-Wall', '-Wextra', '-Werror',
                     '-Wno-deprecated-copy', '-fPIC', *float_flags,
                     str(ROOT/'audb-agent/native/display.cpp'), '-o', str(display), *qt_ui_flags]
    sdk(display_flags)
    # Maliit background plugin: does not replace or patch the stock keyboard.
    module = stage/qml_rel
    module.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT/'audb-agent/native/input/qmldir', module/'qmldir')
    plugin = stage/plugin_rel
    plugin.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT/'audb-agent/native/input/zz-audb-input.qml', plugin)
    source = ROOT/'audb-agent/native/input/bridge.cpp'
    # Generate moc alongside a temporary source copy, never in the source tree.
    native = work/'native-input'
    native.mkdir(exist_ok=True)
    shutil.copy2(source, native/'bridge.cpp')
    plugin_flags = [a.cxx, '-std=c++11', '-shared', '-fPIC', '-O2', '-Wall', '-Wextra',
                    '-Werror', '-Wno-deprecated-copy', *float_flags, 'bridge.cpp', '-o',
                    str(module/'libaudbinput.so'), *qt_plugin_flags]
    compile_cmd = ('cd ' + shlex.quote(str(native)) + ' && moc bridge.cpp -o bridge.moc && '
                   + ' '.join(shlex.quote(flag) for flag in plugin_flags))
    sdk(['/bin/sh', '-c', compile_cmd])

    # The packaged payload is what matters: rpmbuild postprocessing runs after staging, so the
    # ELF checks below run on the files extracted from the RPM, for every platform.

    archive = work/'audb-agent-stage.tar.gz'
    with tarfile.open(archive, 'w:gz') as tar:
        tar.add(stage/'usr', arcname='usr')
    # Build in the container filesystem, as in StrongSwanPortation; sb2 maps shared host paths
    # differently when rpmbuild executes target postprocessors. A fresh topdir per build means a
    # stale RPM cannot be mistaken for this build's and concurrent builds of the same architecture
    # do not delete each other's output.
    top = capture(['docker', 'exec', '--user', 'mersdk', a.container, 'mktemp', '-d',
                   '/home/mersdk/audb-agent-rpmbuild-XXXXXX']).strip()
    run(['docker', 'exec', '--user', 'mersdk', a.container, 'mkdir', '-p', top+'/SOURCES'])
    run(['docker', 'cp', archive, a.container+':'+top+'/SOURCES/audb-agent-stage.tar.gz'])
    run(['docker', 'cp', ROOT/'packaging/audb-agent.spec', a.container+':'+top+'/audb-agent.spec'])
    run(['docker', 'exec', a.container, 'chown', '-R', 'mersdk:mersdk', top])
    sdk(['rpmbuild', *rpm_target_args, '-bb', '--define', '_topdir '+top, '--define', '_enable_debug_packages 0', '--define', 'debug_package %{nil}', top+'/audb-agent.spec'])

    # 3. Discover what rpmbuild actually produced rather than predicting its layout, then assert
    #    the architecture and the packaged paths.
    version = next(l.split(':', 1)[1].strip() for l in (ROOT/'packaging/audb-agent.spec').read_text().splitlines() if l.startswith('Version:'))
    release = next(l.split(':', 1)[1].strip() for l in (ROOT/'packaging/audb-agent.spec').read_text().splitlines() if l.startswith('Release:'))
    listing = capture(['docker', 'exec', a.container, 'find', top+'/RPMS', '-name', '*.rpm', '-type', 'f']).split()
    if len(listing) != 1:
        raise SystemExit(f'expected exactly one RPM under {top}/RPMS, found: {listing}')
    built = listing[0]
    if not Path(built).name.endswith('.' + arch + '.rpm'):
        raise SystemExit(f'{built} is not labelled {arch}')
    output = work/('audb-agent-'+version+'-'+release+'.'+arch+'.rpm')
    candidate = output.with_name(output.name + '.candidate')
    run(['docker', 'cp', a.container+':'+built, candidate])
    # A transfer that silently changes bytes would ship a package whose payload digest no longer
    # matches its header (seen once with a same-size copy), so compare digests with the original
    # and let the target's rpm verify the copy before anything signs or installs it.
    container_md5 = sdk_output(['md5sum', built]).split()[0]
    host_md5 = sdk_output(['md5sum', str(candidate)]).split()[0]
    if container_md5 != host_md5:
        raise SystemExit(f'{candidate} differs from the built RPM ({host_md5} != {container_md5}); refusing to use it')
    # This gate is for the UNSIGNED artefact only: rpmsign-external rewrites the file into the
    # external-signature layout, after which rpmkeys reports payload digests as BAD even on the
    # device's own rpm, so it must never be reused after signing.
    checksig = sdk_output(['rpmkeys', '--checksig', '-v', str(candidate)])
    lines = [line.strip() for line in checksig.splitlines()]
    bad = [line for line in lines if 'BAD' in line or 'NOT OK' in line]
    digest_ok = [line for line in lines if line.endswith('digest: OK')]
    if bad or not digest_ok:
        raise SystemExit(f'{candidate} did not verify as an unsigned package: {bad or "no OK digest line"}')
    # Inspect the copied artifact through the container's rpm; the macOS host has none.
    header = sdk_output(['rpm', '-qp', '--qf', '%{ARCH}', str(candidate)]).strip()
    if header != arch:
        raise SystemExit(f'{candidate} has RPM architecture {header!r}, expected {arch!r}')
    payload = sdk_output(['rpm', '-qpl', str(candidate)]).split()
    for expected in ['/'+str(qml_rel), '/'+str(plugin_rel), '/usr/lib/systemd/system/audb-agent.service']:
        if not any(entry == expected or entry.startswith(expected + '/') for entry in payload):
            raise SystemExit(f'{expected} missing from {candidate}')
    lib64 = [entry for entry in payload if entry.startswith('/usr/lib64/')]
    if lib64 and libdir == '/usr/lib':
        raise SystemExit(f'{candidate} contains lib64 paths although the device contract is {libdir}: {lib64}')
    # Every shipped ELF, validated in the packaged payload rather than the staging tree.
    payload_dir = '/tmp/audb-payload-' + arch
    sdk_output(['/bin/sh', '-c', 'rm -rf ' + payload_dir + ' && mkdir -p ' + payload_dir
                + ' && cd ' + payload_dir + ' && rpm2cpio ' + shlex.quote(str(candidate))
                + ' | cpio -idm --quiet'])
    class_expected, machine_expected = ELF_EXPECT[arch]
    for relative, executable in [('usr/sbin/audb-agent', True), ('usr/bin/audb-agentctl', True),
                                 ('usr/libexec/audb-agent/display', True),
                                 (str(qml_rel/'libaudbinput.so'), False)]:
        path = f'{payload_dir}/{relative}'
        header_text = sdk_output(['readelf', '-h', path])
        if class_expected not in header_text or machine_expected not in header_text:
            raise SystemExit(f'{relative} in the package is not {class_expected}/{machine_expected}')
        if arch == 'armv7hl':
            if not vfp_args_ok(sdk_output(['readelf', '-A', path])):
                raise SystemExit(f'{relative} is not Tag_ABI_VFP_args: VFP registers; the compiler is not hard-float')
            if executable and not interpreter_ok(sdk_output(['readelf', '-l', path])):
                raise SystemExit(f'{relative} does not request /{ARMHF_INTERPRETER}')
    candidate.replace(output)
    run(['docker', 'exec', a.container, 'rm', '-rf', top])
    receipt = {
        'platform': a.platform, 'target': a.target, 'rustTarget': a.rust_target, 'arch': arch,
        'libdir': libdir, 'cc': a.cc, 'cxx': a.cxx, 'sysroot': a.sysroot,
        'rpmTarget': a.rpm_target, 'machine': machine,
        'containerOutputDir': str(Path(built).parent),
        'containerOutputName': Path(built).name, 'hostPath': str(output),
        'rpmArchHeader': header, 'payload': sorted(payload),
    }
    (work/'build-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print('Unsigned RPM: '+str(output), flush=True)
    print(json.dumps(receipt, indent=2), flush=True)


if __name__ == '__main__':
    sys.exit(main())
