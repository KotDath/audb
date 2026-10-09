Name: audb-agent
Version: 0.3.0
Release: 10
Summary: Device automation service for Aurora Debug Bridge
License: MIT
Source0: audb-agent-stage.tar.gz
Requires: systemd
Requires: jolla-keyboard
Requires: /usr/bin/dbus-send

%description
Rust input, screenshot and permission service and client, with Qt geometry query. The local control
socket permits root and members of the system input group to inject touchscreen
events. Intended for developer devices, installed by audb setup-device.

%prep
%setup -q -c -T
tar -xzf %{SOURCE0}

%build
# Cross-built by scripts/build-agent-rpm.py using the Aurora SDK target linker.

%install
mkdir -p %{buildroot}
cp -a usr %{buildroot}/

%post
systemctl daemon-reload
systemctl enable audb-agent.service
systemctl restart audb-agent.service

%preun
if [ "$1" -eq 0 ]; then
    systemctl disable --now audb-agent.service || :
fi

%postun
systemctl daemon-reload || :

%files
%defattr(-,root,root,-)
/usr/sbin/audb-agent
/usr/bin/audb-agentctl
/usr/libexec/audb-agent
/usr/lib64/qt5/qml/Audb
/usr/lib64/maliit/plugins/zz-audb-input.qml
/usr/lib/systemd/system/audb-agent.service

%license /usr/share/licenses/audb-agent/LICENSE
