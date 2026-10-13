#!/usr/bin/env python3
"""Cross-build audb-agent and package it using Aurora SDK sb2/rpmbuild.
Signing is a separate step. Requires installed Rust target and running SDK engine.

Everything architecture-specific is derived from the *selected sb2 target's own rpm*
(`%{_target_cpu}`, `%{_libdir}`) and asserted against the device contract below, so a
mistyped --target/--rust-target fails the build instead of producing a plausible-looking
RPM for the wrong platform. `--self-test` checks that logic without Docker.
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

# Device contract per rpm architecture: where Qt QML modules and Maliit plugins are
# actually resolved on the device. /usr/lib/systemd/system is deliberately *not* here:
# it is not %{_libdir} and stays literal in the spec.
PLATFORM_LIBDIR = {'aarch64': '/usr/lib64', 'armv7hl': '/usr/lib'}
# Aurora's own armv7hl optflags (rpmrc): Armv7-A, hard-float ABI, VFPv3-D16 (not NEON).
ARM_FLOAT_FLAGS = ['-march=armv7-a', '-mfloat-abi=hard', '-mfpu=vfpv3-d16']
ARMHF_INTERPRETER = 'ld-linux-armhf.so.3'


def expected_machine(arch):
    """The `gcc -dumpmachine` prefix that must produce this rpm architecture."""
    return {'aarch64': 'aarch64', 'armv7hl': 'armv7'}[arch]


def parse_target_platform(output):
    """Parses `rpm --eval '%{_target_cpu} %{_arch} %{_libdir}'` run inside an sb2 target.

    rpm echoes macros it cannot resolve verbatim, so an unresolved field must never be
    mistaken for a value.
    """
    fields = output.split()
    if len(fields) != 3 or any('%' in f or '{' in f or not f for f in fields):
        raise SystemExit(f'Unresolved rpm platform probe inside the sb2 target: {output!r}')
    arch, libdir = fields[0], fields[2]
    contract = PLATFORM_LIBDIR.get(arch)
    if contract is None:
        raise SystemExit(f'No device libdir contract for rpm architecture {arch!r}')
    if libdir != contract:
        raise SystemExit(
            f'sb2 target reports {arch} with libdir {libdir}, but the device contract for '
            f'{arch} is {contract}; refusing to build for the wrong platform'
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
    print('self-test: ok')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--container', default=os.environ.get('AUDB_SDK_CONTAINER', 'aurora-os-build-engine-5.2.1.200-mb2_kotdath'))
    p.add_argument('--target', default='AuroraOS-5.2.1.200-aarch64', help='sb2 target that runs the compilation and rpmbuild')
    p.add_argument('--rust-target', default='aarch64-unknown-linux-gnu')
    p.add_argument('--arch', default=None, help='Assertion only: expected RPM architecture. Never selects paths.')
    p.add_argument('--self-test', action='store_true', help='Check the architecture logic without Docker')
    a = p.parse_args()
    if a.self_test:
        return self_test()

    def capture(argv, **kw):
        print(shlex.join(map(str, argv)), flush=True)
        return subprocess.run(list(map(str, argv)), check=True, cwd=ROOT, text=True, stdout=subprocess.PIPE, **kw).stdout

    def run(argv, **kw):
        print(shlex.join(map(str, argv)), flush=True)
        return subprocess.run(list(map(str, argv)), check=True, cwd=ROOT, **kw)

    def sdk(argv):
        return run(['docker', 'exec', '--user', 'mersdk', a.container, 'sb2', '-t', a.target, '-m', 'sdk-build', *argv])

    def sdk_output(argv):
        return capture(['docker', 'exec', '--user', 'mersdk', a.container, 'sb2', '-t', a.target, '-m', 'sdk-build', *argv])

    # 1. Ask the target's own rpm what it builds for, and fail closed on any disagreement
    #    with the device contract. `%{_libdir}` is an install path inside the payload and
    #    is never used as a host or sb2 filesystem path.
    arch, libdir = parse_target_platform(sdk_output(['rpm', '--eval', '%{_target_cpu} %{_arch} %{_libdir}']))
    check_rust_target(arch, a.rust_target)
    if a.arch is not None and a.arch != arch:
        raise SystemExit(f'--arch {a.arch} contradicts the sb2 target, which builds {arch}')
    machine = sdk_output(['gcc', '-dumpmachine']).strip()
    if not machine.startswith(expected_machine(arch)):
        raise SystemExit(
            f'sb2 target {a.target} reports gcc machine {machine}, expected {expected_machine(arch)}-*; '
            f'the target is missing or was not selected'
        )
    qml_rel, plugin_rel = install_paths(libdir)
    float_flags = ARM_FLOAT_FLAGS if arch == 'armv7hl' else []
    print(f'building {arch} with libdir {libdir} via {machine}', flush=True)

    env = os.environ.copy()
    env['AUDB_SDK_CONTAINER'] = a.container
    env['AUDB_SDK_TARGET'] = a.target
    env['RUSTC'] = subprocess.check_output(['rustup', 'which', '--toolchain', 'stable', 'rustc'], text=True).strip()
    env['CARGO_TARGET_' + a.rust_target.upper().replace('-', '_') + '_LINKER'] = str(ROOT/'scripts/aurora-linker.sh')
    # The linker wrapper re-checks the sb2 machine, so a Cargo triple aimed at the wrong
    # target cannot silently link against another platform's libc.
    env['AUDB_EXPECT_MACHINE'] = expected_machine(arch)
    if float_flags:
        # Link arguments, not bare RUSTFLAGS: the Aurora gcc driver is what selects the
        # hard-float sysroot and emits the ABI attribute.
        env['CARGO_TARGET_' + a.rust_target.upper().replace('-', '_') + '_RUSTFLAGS'] = ' '.join(
            shlex.quote('-C link-arg=' + flag) for flag in float_flags
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
    # pkg-config must run within sb2, so compiler flags refer to the target headers.
    flags = ' '.join(float_flags)
    compile_cmd = f'g++ -std=c++11 -O2 -Wall -Wextra -Werror -Wno-deprecated-copy -fPIC {flags} ' + shlex.quote(str(ROOT/'audb-agent/native/display.cpp')) + ' -o ' + shlex.quote(str(display)) + ' $(pkg-config --cflags --libs Qt5Gui Qt5Core)'
    sdk(['/bin/sh', '-c', compile_cmd])
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
    compile_cmd = 'cd ' + shlex.quote(str(native)) + ' && moc bridge.cpp -o bridge.moc && g++ -std=c++11 -shared -fPIC -O2 -Wall -Wextra -Werror -Wno-deprecated-copy ' + flags + ' bridge.cpp -o ' + shlex.quote(str(module/'libaudbinput.so')) + ' $(pkg-config --cflags --libs Qt5Quick Qt5Qml Qt5Network)'
    sdk(['/bin/sh', '-c', compile_cmd])

    # 2. Every shipped ELF must use the platform's float ABI, not just the Rust ones: the
    #    plugin is what Maliit dlopens and it is built by sb2 g++, which never passes
    #    through scripts/aurora-linker.sh. Checked on the staged (stripped) files.
    if arch == 'armv7hl':
        elf_checks = [
            ('usr/sbin/audb-agent', True), ('usr/bin/audb-agentctl', True),
            ('usr/libexec/audb-agent/display', True), (str(qml_rel/'libaudbinput.so'), False),
        ]
        for relative, executable in elf_checks:
            path = str(stage/relative)
            attributes = sdk_output(['readelf', '-A', path])
            if not vfp_args_ok(attributes):
                raise SystemExit(f'{relative} is not Tag_ABI_VFP_args: VFP registers; the target gcc is not hard-float')
            if executable:
                headers = sdk_output(['readelf', '-l', path])
                if not interpreter_ok(headers):
                    raise SystemExit(f'{relative} does not request /{ARMHF_INTERPRETER}')

    archive = work/'audb-agent-stage.tar.gz'
    with tarfile.open(archive, 'w:gz') as tar:
        tar.add(stage/'usr', arcname='usr')
    # Build in the native engine filesystem, as in StrongSwanPortation; sb2 maps
    # shared host paths differently when rpmbuild executes target postprocessors.
    # The topdir is removed first so a stale RPM cannot be mistaken for this build's.
    top = '/home/mersdk/audb-agent-rpmbuild-' + arch
    run(['docker', 'exec', a.container, 'rm', '-rf', top])
    run(['docker', 'exec', '--user', 'mersdk', a.container, 'mkdir', '-p', top+'/SOURCES'])
    run(['docker', 'cp', archive, a.container+':'+top+'/SOURCES/audb-agent-stage.tar.gz'])
    run(['docker', 'cp', ROOT/'packaging/audb-agent.spec', a.container+':'+top+'/audb-agent.spec'])
    run(['docker', 'exec', a.container, 'chown', '-R', 'mersdk:mersdk', top])
    sdk(['rpmbuild', '-bb', '--define', '_topdir '+top, '--define', '_enable_debug_packages 0', '--define', 'debug_package %{nil}', top+'/audb-agent.spec'])

    # 3. Discover what rpmbuild actually produced rather than predicting its layout, then
    #    assert the architecture and the packaged paths.
    version = next(l.split(':', 1)[1].strip() for l in (ROOT/'packaging/audb-agent.spec').read_text().splitlines() if l.startswith('Version:'))
    release = next(l.split(':', 1)[1].strip() for l in (ROOT/'packaging/audb-agent.spec').read_text().splitlines() if l.startswith('Release:'))
    listing = capture(['docker', 'exec', a.container, 'find', top+'/RPMS', '-name', '*.rpm', '-type', 'f']).split()
    if len(listing) != 1:
        raise SystemExit(f'expected exactly one RPM under {top}/RPMS, found: {listing}')
    built = listing[0]
    if not Path(built).name.endswith('.' + arch + '.rpm'):
        raise SystemExit(f'{built} is not labelled {arch}')
    output = work/('audb-agent-'+version+'-'+release+'.'+arch+'.rpm')
    run(['docker', 'cp', a.container+':'+built, output])
    # Inspect the copied artifact through the target's own rpm; the host may have none.
    header = sdk_output(['rpm', '-qp', '--qf', '%{ARCH}', str(output)]).strip()
    if header != arch:
        raise SystemExit(f'{output} has RPM architecture {header!r}, expected {arch!r}')
    payload = sdk_output(['rpm', '-qpl', str(output)]).split()
    for expected in ['/'+str(qml_rel), '/'+str(plugin_rel), '/usr/lib/systemd/system/audb-agent.service']:
        if not any(entry == expected or entry.startswith(expected + '/') for entry in payload):
            raise SystemExit(f'{expected} missing from {output}')
    lib64 = [entry for entry in payload if entry.startswith('/usr/lib64/')]
    if lib64 and libdir == '/usr/lib':
        raise SystemExit(f'{output} contains lib64 paths although the device contract is {libdir}: {lib64}')
    receipt = {
        'target': a.target, 'rustTarget': a.rust_target, 'arch': arch, 'libdir': libdir,
        'gccMachine': machine, 'containerOutputDir': str(Path(built).parent),
        'containerOutputName': Path(built).name, 'hostPath': str(output),
        'rpmArchHeader': header, 'payload': sorted(payload),
    }
    (work/'build-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print('Unsigned RPM: '+str(output), flush=True)
    print(json.dumps(receipt, indent=2), flush=True)


if __name__ == '__main__':
    sys.exit(main())
