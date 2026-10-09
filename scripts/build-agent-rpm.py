#!/usr/bin/env python3
"""Cross-build audb-agent and package it using Aurora SDK sb2/rpmbuild.
Signing is a separate step. Requires installed Rust target and running SDK engine.
"""
import argparse
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--container', default=os.environ.get('AUDB_SDK_CONTAINER', 'aurora-os-build-engine-5.2.1.200-mb2_kotdath'))
p.add_argument('--target', default='AuroraOS-5.2.1.200-aarch64')
p.add_argument('--rust-target', default='aarch64-unknown-linux-gnu')
p.add_argument('--arch', default='aarch64')
a = p.parse_args()

def run(argv, **kw):
    print(shlex.join(map(str, argv)), flush=True)
    return subprocess.run(list(map(str, argv)), check=True, cwd=ROOT, **kw)

def sdk(argv):
    return run(['docker','exec','--user','mersdk',a.container,'sb2','-t',a.target,'-m','sdk-build',*argv])

env = os.environ.copy()
env['AUDB_SDK_CONTAINER'] = a.container
env['AUDB_SDK_TARGET'] = a.target
env['RUSTC'] = subprocess.check_output(['rustup','which','--toolchain','stable','rustc'], text=True).strip()
env['CARGO_TARGET_' + a.rust_target.upper().replace('-','_') + '_LINKER'] = str(ROOT/'scripts/aurora-linker.sh')
run(['rustup','run','stable','cargo','build','-p','audb-agent','--release','--target',a.rust_target], env=env)
work = ROOT/'target'/'agent-rpm'/a.arch
stage = work/'stage'
if stage.exists():
    shutil.rmtree(stage)
for binary, destination in [('audb-agent','usr/sbin/audb-agent'),('audb-agentctl','usr/bin/audb-agentctl')]:
    dest = stage/destination
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT/'target'/a.rust_target/'release'/binary,dest)
license_file=stage/'usr/share/licenses/audb-agent/LICENSE'
license_file.parent.mkdir(parents=True,exist_ok=True)
shutil.copy2(ROOT/'LICENSE',license_file)
service = stage/'usr/lib/systemd/system/audb-agent.service'
service.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(ROOT/'packaging/audb-agent.service', service)
display = stage/'usr/libexec/audb-agent/display'
display.parent.mkdir(parents=True, exist_ok=True)
# pkg-config must run within sb2, so compiler flags refer to the target headers.
compile_cmd = 'g++ -std=c++11 -O2 -Wall -Wextra -Werror -Wno-deprecated-copy -fPIC ' + shlex.quote(str(ROOT/'audb-agent/native/display.cpp')) + ' -o ' + shlex.quote(str(display)) + ' $(pkg-config --cflags --libs Qt5Gui Qt5Core)'
sdk(['/bin/sh','-c',compile_cmd])
# Maliit background plugin: does not replace or patch the stock keyboard.
module = stage/'usr/lib64/qt5/qml/Audb/Input'
module.mkdir(parents=True,exist_ok=True)
shutil.copy2(ROOT/'audb-agent/native/input/qmldir',module/'qmldir')
plugin = stage/'usr/lib64/maliit/plugins/zz-audb-input.qml'
plugin.parent.mkdir(parents=True,exist_ok=True)
shutil.copy2(ROOT/'audb-agent/native/input/zz-audb-input.qml',plugin)
source = ROOT/'audb-agent/native/input/bridge.cpp'
# Generate moc alongside a temporary source copy, never in the source tree.
native = work/'native-input'
native.mkdir(exist_ok=True)
shutil.copy2(source,native/'bridge.cpp')
compile_cmd = 'cd '+shlex.quote(str(native))+' && moc bridge.cpp -o bridge.moc && g++ -std=c++11 -shared -fPIC -O2 -Wall -Wextra -Werror -Wno-deprecated-copy bridge.cpp -o '+shlex.quote(str(module/'libaudbinput.so'))+' $(pkg-config --cflags --libs Qt5Quick Qt5Qml Qt5Network)'
sdk(['/bin/sh','-c',compile_cmd])
archive = work/'audb-agent-stage.tar.gz'
with tarfile.open(archive,'w:gz') as tar:
    tar.add(stage/'usr',arcname='usr')
# Build in the native engine filesystem, as in StrongSwanPortation; sb2 maps
# shared host paths differently when rpmbuild executes target postprocessors.
top = '/home/mersdk/audb-agent-rpmbuild-' + a.arch
run(['docker','exec','--user','mersdk',a.container,'mkdir','-p',top+'/SOURCES'])
run(['docker','cp',archive,a.container+':'+top+'/SOURCES/audb-agent-stage.tar.gz'])
run(['docker','cp',ROOT/'packaging/audb-agent.spec',a.container+':'+top+'/audb-agent.spec'])
run(['docker','exec',a.container,'chown','-R','mersdk:mersdk',top])
sdk(['rpmbuild','-bb','--define','_topdir '+top,'--define','_enable_debug_packages 0','--define','debug_package %{nil}',top+'/audb-agent.spec'])
version = next(l.split(':',1)[1].strip() for l in (ROOT/'packaging/audb-agent.spec').read_text().splitlines() if l.startswith('Version:'))
release = next(l.split(':',1)[1].strip() for l in (ROOT/'packaging/audb-agent.spec').read_text().splitlines() if l.startswith('Release:'))
filename = 'audb-agent-'+version+'-'+release+'.'+a.arch+'.rpm'
output = work/filename
run(['docker','cp',a.container+':'+top+'/RPMS/'+a.arch+'/'+filename,output])
print('Unsigned RPM: '+str(output),flush=True)
