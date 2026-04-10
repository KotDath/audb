# AUDB Emulator Adaptation Plan

## Goal

Этот документ отвечает на узкий вопрос:

- как адаптировать `audb` под новый тип устройства: наш Aurora QEMU emulator;
- какой backend должна использовать каждая команда;
- какие изменения нужны в модели устройства и в server runtime.

Это не общий план для физических устройств. Это canonical plan именно для emulator path.

Non-goal:

- не убирать поддержку реальных Aurora-устройств;
- не заменять physical-device backend на emulator backend;
- не делать `QMP` обязательным для обычных устройств.

## Canonical Emulator Profile

Для `audb` нужно считать canonical не stock SDK launcher, а вручную запущенный hybrid/no-GL профиль, который уже подтверждён как рабочий:

- QEMU with:
  - `virtio-tablet-pci`
  - `virtio-multitouch-pci`
  - `virtio-vga`
  - `-display sdl,show-cursor=off`
- host-side `QMP` socket:
  - `/tmp/aurora-screendump.qmp`
- host-side SSH forward:
  - `127.0.0.1:33223`
- guest SSH key:
  - `/home/kotdath/AuroraOS/vmshare/ssh/private_keys/sdk`

Почему именно он:

- `tap` через `QMP` работает;
- `swipe` через `QMP mtt` работает;
- `screendump` через `QMP` работает;
- stock SDK emulator с `virtio-tablet-pci` не даёт надёжного gesture path;
- GL profile даёт input, но ломает `QMP screendump` с `no surface`.

## Main Architectural Change

Сейчас в `audb` поле `platform` одновременно играет роль “что это за устройство” и “под какую архитектуру оно собрано”, но для emulator этого уже недостаточно.

Для emulator нужно разделить:

- архитектуру target:
  - `aurora-arm`
  - `aurora-arm64`
  - `aurora-x86_64`
- тип устройства:
  - `physical`
  - `qemu-emulator`

### Proposed device model

Вместо текущего `Device { host, port, auth, root_password, platform }` нужен смыслово такой слой:

```text
Device
  name
  host
  port
  auth
  arch
  kind
  root_password?
  emulator?
```

Где `emulator` содержит:

```text
QemuEmulatorConfig
  qmp_socket
  viewport_width
  viewport_height
  abs_max
  preferred_input_backend
  preferred_screenshot_backend
```

Canonical defaults для нашего emulator:

- `arch = aurora-x86_64`
- `kind = qemu-emulator`
- `host = 127.0.0.1`
- `port = 33223`
- `qmp_socket = /tmp/aurora-screendump.qmp`
- `viewport_width = 360`
- `viewport_height = 800`
- `abs_max = 32767`
- `preferred_input_backend = qmp-multitouch`
- `preferred_screenshot_backend = qmp-screendump`

## Runtime Split: SSH vs QMP

Для emulator нельзя всё вести через один guest-side backend.

Нужны два execution path:

### 1. Guest path over SSH

Используется для команд, которые по смыслу должны выполняться внутри Aurora guest:

- `shell`
- `push`
- `pull`
- `package install`
- `package uninstall`
- `package list`
- `info`
- `launch`
- `stop`
- `logs`
- `open`
- часть `key`

### 2. Host path over QMP

Используется для команд, которые должны работать через QEMU surface/input stack:

- `tap`
- `swipe`
- `screenshot`
- часть `key`

То есть для emulator canonical rule такая:

- app/system operations идут в guest;
- UI automation идёт в host-side QMP.

Для physical devices canonical rule остаётся другой:

- guest-side operations идут по SSH через `audb-server`;
- input/screenshot продолжают использовать `AudbBridge` на устройстве;
- никакой `QMP` в physical-device path не появляется.

## Command-by-Command Adaptation

### `audb device list`

Что менять:

- показывать не только `host:port`, но и `kind`, `arch`, `qmp_socket` для emulator;
- отдельно отображать capability state:
  - `ssh=ok/fail`
  - `qmp=ok/fail`

Как реализовывать:

- локальный config store остаётся;
- для emulator entry server-status должен собирать два health check:
  - SSH reachability;
  - existence/connectivity of QMP socket.

### `audb device add`

Что менять:

- текущий interactive flow недостаточен;
- нужен отдельный сценарий добавления emulator device.

Как реализовывать:

- добавить `audb device add-emulator` или equivalent interactive branch;
- спрашивать:
  - name
  - SSH host
  - SSH port
  - SSH key
  - QMP socket
  - viewport width/height
- автоматически подставлять canonical defaults для нашего профиля.

Что валидировать при добавлении:

- SSH connect to `defaultuser`;
- QMP socket exists and отвечает на `qmp_capabilities`;
- при желании `query-commands` содержит `input-send-event` и `screendump`.

### `audb device remove`

Что менять:

- только формат отображения и удаление нового emulator config.

Как реализовывать:

- локально, как и сейчас;
- если удаляется emulator, сервер должен забывать и SSH session, и QMP metadata.

### `audb select`

Что менять:

- selection должна выбирать не просто `host`, а полноценный device profile.

Как реализовывать:

- локально;
- хранить selection по device id или name, а не по голому host.

Почему это важно:

- у emulator и физического устройства может быть один и тот же `127.0.0.1`/forwarded host pattern;
- host alone перестаёт быть хорошим identity key.

### `audb ping`

Что менять:

- сам `ping` можно оставить как server ping;
- для emulator полезнее capability-aware status, чем просто `pong`.

Как реализовывать:

- не менять базовую семантику `ping`;
- emulator health показывать в `server-status` и `device list`.

### `audb start-server`

Что менять:

- server при старте должен уметь загружать emulator metadata, а не только SSH devices.

Как реализовывать:

- оставить daemon behavior как есть;
- при загрузке devices инициализировать для emulator:
  - SSH connection queue;
  - lazy QMP metadata object.

### `audb kill-server`

Что менять:

- почти ничего.

Как реализовывать:

- оставить PID-based local kill;
- emulator-specific cleanup не нужен, потому что QEMU process не является child `audb-server`.

### `audb server-status`

Что менять:

- для emulator нужно расширить статус.

Как реализовывать:

- добавить в server status fields:
  - `device_kind`
  - `arch`
  - `qmp_connected`
  - `input_backend`
  - `screenshot_backend`
- для emulator показывать:
  - SSH state from pool
  - QMP readiness from lightweight probe

### `audb shell`

Что менять:

- ничего по сути.

Как реализовывать:

- оставить через SSH/persistent session;
- emulator для `shell` это просто Aurora guest на `127.0.0.1:33223`.

### `audb push`

Что менять:

- ничего по сути.

Как реализовывать:

- оставить через SSH/SFTP.

### `audb pull`

Что менять:

- ничего по сути.

Как реализовывать:

- оставить через SSH/SFTP.

### `audb package install`

Что менять:

- логика инсталла остаётся той же;
- но device arch для emulator должен быть `x86_64`.

Как реализовывать:

- тот же `APM.Install` через SSH;
- перед install добавить preflight check на `arch`, если появится package metadata parsing.

Практический смысл:

- для emulator нельзя продолжать считать все targets `aurora-arm` или `aurora-arm64`;
- иначе пользователь будет пытаться ставить не тот RPM.

### `audb package uninstall`

Что менять:

- ничего по backend.

Как реализовывать:

- оставить через guest-side `APM.Remove`.

### `audb package list`

Что менять:

- ничего по backend.

Как реализовывать:

- оставить через guest-side `APM.GetPackageList`.

### `audb package sign`

Что менять:

- sign local-only, но для emulator надо учитывать x86_64 output artifact.

Как реализовывать:

- сам sign pipeline не менять;
- в будущем build/sign pipeline должен брать `aurora-x86_64` target, если выбран emulator device.

### `audb package validate`

Что менять:

- почти ничего.

Как реализовывать:

- local Docker validation as-is;
- при наличии arch-aware build flow просто валидировать уже x86_64 RPM.

### `audb info`

Что менять:

- backend остаётся guest-side;
- в output полезно отдельно показывать, что это emulator.

Как реализовывать:

- собирать системную информацию по SSH как сейчас;
- дополнительно добавлять emulator-only synthetic block:
  - `device_kind = qemu-emulator`
  - `qmp_socket`
  - `viewport = 360x800`
  - `automation = qmp-multitouch`

### `audb tap`

Что менять:

- для emulator canonical backend должен быть `QMP multitouch`, не `AudbBridge`.

Почему:

- host-side `QMP mtt` уже проверен и даёт реальные touch semantics;
- это не требует guest-side helper app;
- это не зависит от того, как внутри guest маршрутизирован libinput/plugin stack.

Как реализовывать:

- в server добавить emulator-aware input executor;
- если `kind = qemu-emulator`, вместо SSH command:
  - connect to QMP socket;
  - send `input-send-event` with `mtt(begin/data/end)` sequence;
  - convert px to abs with `32767 / (width-1)` formula.

Canonical conversion:

```text
abs_x = round(x_px * 32767 / 359)
abs_y = round(y_px * 32767 / 799)
```

CLI compatibility:

- существующий `audb tap X Y` сохраняется;
- `--duration` продолжает работать как hold time;
- `--event` для emulator надо считать legacy/no-op или отклонять, потому что canonical backend уже не evdev path inside guest.

### `audb swipe`

Что менять:

- для emulator canonical backend тоже должен быть `QMP multitouch`.

Почему:

- `virtio-multitouch-pci` + `QMP mtt` уже даёт рабочие swipe gestures;
- stock tablet-only path не годится.

Как реализовывать:

- в emulator mode direction swipe и coordinate swipe преобразуются в host-side `input-send-event`;
- использовать уже подтверждённую последовательность:
  - `mtt(begin)`
  - `btn(touch,true)`
  - `mtt(data x/y)`
  - repeated `mtt(update) + btn + data`
  - `mtt(end)`

Что ещё добавить:

- на emulator есть смысл расширить CLI `swipe` опциями:
  - `--steps`
  - `--duration-ms`
  - `--hold-ms`
- потому что QMP gesture timing реально влияет на UI.

### `audb key`

Для emulator эта команда должна стать mixed-backend.

#### `power`

Как реализовывать:

- оставить guest-side command через `com.nokia.mce.request.req_trigger_powerkey_event`.

Почему:

- это системный semantic action;
- не требует моделировать физическую клавишу через QEMU input.

#### `lock` / `unlock`

Как реализовывать:

- оставить guest-side `mce` D-Bus path как сейчас.

#### `home` / `back` / `menu` / `close`

Как реализовывать:

- для emulator реализовывать как QMP swipe macros, не как bridge gestures в guest.

Mapping:

- `home` -> upward bottom-edge swipe
- `close` -> same as `home` in current Aurora navigation model
- `back` -> edge swipe from left to right
- `menu` -> downward or app-specific gesture only if реально нужна и подтверждена

Почему:

- в текущем codebase эти “keys” уже по сути являются gestures;
- на emulator gestures надёжнее делать host-side QMP.

#### `volumeup` / `volumedown`

Как реализовывать:

- не использовать текущий hardcoded `/dev/input/event1` path из `AudbBridge/helpercli.cpp`, потому что в hybrid VM:
  - `event1` = tablet
  - `event2` = multitouch
  - `event3` = keyboard
- для emulator добавить отдельный keyboard backend:
  - либо `QMP` keyboard events;
  - либо guest-side event injection into the real keyboard device after explicit detection.

Canonical choice:

- лучше сделать host-side `QMP` keyboard backend, чтобы не зависеть от guest event numbering.

### `audb screenshot`

Что менять:

- для emulator canonical backend должен быть `QMP screendump`, не `AudbBridge`, не lipstick D-Bus.

Почему:

- рабочий профиль уже подтверждён;
- host-side `QMP screendump` на no-GL hybrid VM создаёт PNG;
- guest-side screenshot paths зависят от permissions и compositor internals.

Как реализовывать:

- если `kind = qemu-emulator`:
  - connect to QMP;
  - call `screendump` with `format=png`;
  - return binary to client or write temp file then read it.

Важное условие:

- этот backend должен использоваться только на canonical no-GL profile;
- на GL profile нужно считать screenshot unsupported или fallback.

### `audb launch`

Что менять:

- ничего по backend.

Как реализовывать:

- оставить guest-side RuntimeManager `Start`.

### `audb stop`

Что менять:

- ничего по backend.

Как реализовывать:

- оставить guest-side RuntimeManager `Terminate`.

### `audb logs`

Что менять:

- ничего по backend.

Как реализовывать:

- оставить guest-side `journalctl`.

### `audb reconnect`

Что менять:

- для emulator reconnect должен сбрасывать не только SSH session, но и QMP session cache.

Как реализовывать:

- `Reconnect { device }` for emulator:
  - drop persistent SSH handle;
  - drop cached QMP connection/client object if он будет храниться;
  - next command reconnects both lazily.

### `audb open`

Что менять:

- ничего по backend.

Как реализовывать:

- оставить guest-side `org.sailfishos.fileservice.openUrl`.

## What Must Change in Code

### 1. Device schema

Нужно:

- заменить или расширить `Platform`;
- добавить `aurora-x86_64`;
- добавить `DeviceKind`;
- добавить `QemuEmulatorConfig`.

### 2. Device add UX

Нужно:

- новый emulator-specific add flow;
- автозаполнение canonical values;
- проверка `QMP` alongside SSH.

### 3. Server execution layer

Нужно:

- отдельный emulator executor beside SSH pool;
- or at least emulator-aware branches inside command handlers.

Практически:

- `Tap`
- `Swipe`
- `Screenshot`
- часть `Key`

должны в server dispatch идти не в `pool.execute_command(...)`, а в `qmp_*` helpers.

### 4. Protocol

Минимально можно оставить старый protocol shape и branch by device kind on server.

Но лучше добавить emulator-aware option fields:

- for `Swipe`:
  - steps
  - duration_ms
  - hold_ms
- for `Key`:
  - maybe backend override later

### 5. Capability probing

Для emulator нужно ввести явные capability checks:

- `ssh_ok`
- `qmp_ok`
- `qmp_input_ok`
- `qmp_screendump_ok`

Иначе пользователь будет получать поздние runtime errors вместо понятного device status.

## Commands That Stay Unchanged on Emulator

Эти команды по сути не требуют специального emulator backend, кроме device metadata:

- `shell`
- `push`
- `pull`
- `package install`
- `package uninstall`
- `package list`
- `package sign`
- `package validate`
- `info`
- `launch`
- `stop`
- `logs`
- `open`

## Commands That Must Become Emulator-Aware

- `device add`
- `device list`
- `select`
- `server-status`
- `tap`
- `swipe`
- `key`
- `screenshot`
- `reconnect`

## Coexistence Rule

Итоговая модель должна быть branch-by-device-kind, а не replacement:

- `physical` device:
  - `tap/swipe/key/screenshot` через `AudbBridge`
  - остальное через SSH
- `qemu-emulator` device:
  - `tap/swipe/screenshot` через `QMP`
  - `power/lock/unlock` и app/system operations через SSH/guest D-Bus
  - gesture-like keys через `QMP`

То есть новый emulator backend должен быть additive.
Он не должен ломать и не должен вытеснять поддержку реальных устройств.

## Final Canonical Rule Set For Emulator

- Emulator is a first-class device kind, not a hacked physical-device profile.
- Architecture must be explicit: `aurora-x86_64`.
- SSH is for in-guest operations.
- `QMP` is for UI automation and screenshots.
- `AudbBridge` is no longer the canonical automation backend for emulator input.
- `QMP screendump` is the canonical screenshot backend for emulator.
- Stock SDK tablet-only launcher is not the target profile for `audb`.
- The target profile is the confirmed no-GL hybrid launcher with:
  - multitouch
  - QMP touch
  - QMP screendump
