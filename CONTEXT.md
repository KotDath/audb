# CONTEXT

Рабочий каталог: `/home/kotdath/omp/personal/rust/audb`

Дата сессии: `2026-04-01`

## Задача

Нужно довести до рабочего состояния `Screenshot` в `AudbBridge` для Aurora OS emulator.

Исходная гипотеза подтвердилась: простой вызов `ScreenGrab` через D-Bus и чтение `videopipe` как обычного файла не работает, потому что API отдаёт не PNG/JPEG, а потоковый формат, который нужно потреблять через `StreamCamera` или корректно декодировать как видеопоток.

## Что уже было известно от пользователя

- Тап и свайп уже реализуются через кастомный инстанс эмулятора.
- Для скриншотов пользователь начал реализацию через `ScreenGrab`.
- На живом эмуляторе ранее было проверено:
  - `Ping` работает.
  - `Screenshot` не работает.
  - Ошибка была такой:

```text
Screen grab failed: Unsupported videopipe payload, size=20, prefix=02 00 02 00 00 00 d0 02 00 00 40 06 00 00 01 00; fallback failed: Screenshot failed: PID ... is not in privileged group
```

- Из этого следует:
  - `ScreenGrab` реально отвечает.
  - `RequestVideoPipe()` не игнорируется.
  - Отдаётся не готовая картинка, а собственный payload видеопотока.

## Где брать документацию по Aurora

Пользователь сначала просил обращаться к `search_docs` у `aurora-grimoire`, потом уточнил, что для этой задачи нужно искать напрямую здесь:

- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master`

Это важный источник для следующей сессии.

## Что было найдено в документации

### ScreenGrab API

Файл:

- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/screengrab_api.md`

Ключевые выводы:

- Для захвата экрана используется `CameraFacing::Screen`.
- Нужен permission `ScreenCapture`.
- На текущий момент захват экрана поддерживается только через `StreamCamera API`.
- Поддерживаются YUV-форматы:
  - `I420`, если `chromaStep == 1`
  - обычно `NV12`, если `chromaStep == 2`
- Для точной интерпретации нужно опираться на `GraphicBuffer::mapYCbCr()`.

### StreamCamera API

Документация смотрелась в ветке `stream_camera-5.2`.

Использованные файлы:

- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/stream_camera-5.2/streamcamera_8h.md`
- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/stream_camera-5.2/CameraManager.md`
- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/stream_camera-5.2/Camera.md`
- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/stream_camera-5.2/CameraListener.md`
- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/stream_camera-5.2/GraphicBuffer.md`
- `/home/kotdath/omp/personal/rust/aurora-grimoire/thirdparty/dev-documentation-master/documentation/software_development/reference/stream_camera-5.2/YCbCrFrame.md`

Ключевые выводы:

- `CameraFacing` содержит `Screen`.
- `PixelFormat` содержит как минимум:
  - `YCbCrFlexible`
  - `YUV420Planar`
  - `YUV420SemiPlanar`
- `CameraListener::onCameraFrame(std::shared_ptr<GraphicBuffer>)` используется для получения кадров.
- `GraphicBuffer` имеет:
  - `width`
  - `height`
  - `timestampUs`
  - `pixelFormat`
  - `handle`
  - `handleType`
  - методы `mapYCbCr()`, `map()`, `mapFrame()`, `rotation()`
- `YCbCrFrame` имеет:
  - `y`, `cb`, `cr`
  - `yStride`, `cStride`, `chromaStep`
  - `width`, `height`, `timestampUs`
- Если `chromaStep == 1`, это planar `I420`.
- Если `chromaStep == 2`, это, как правило, `NV12`.

## Что было проверено на эмуляторе через D-Bus

Проверялось напрямую на живом эмуляторе, что backend ScreenGrab существует.

Команда:

```bash
dbus-send --session --print-reply \
  --dest=ru.auroraos.ScreenGrab1.Backend \
  /ru/auroraos/ScreenGrab1/Backend \
  org.freedesktop.DBus.Introspectable.Introspect
```

По результату были видны методы:

- `Stop()`
- `GetScreenInfo() -> a{sa{sv}}`
- `RequestVideoPipe(s id, a{sv} params) -> h videopipe_fd`

Также `GetScreenInfo()` возвращал для primary screen размер `720x1600`.

Вывод: текущий `ScreenGrab` API действительно про видеопайп, а не про готовый image blob.

## Permission

Файл:

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/ru.kotdath.AudbBridge.desktop`

В нём уже есть:

```text
Permissions=ScreenCapture
```

То есть permission уже добавлен, это не текущий блокер.

## Какие изменения уже внесены в код

### 1. Новый файл совместимости StreamCamera

Добавлен файл:

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/src/streamcamera_compat.h`

Это локальный минимальный header с ручными объявлениями API `Aurora::StreamCamera`, собранный по документации Aurora 5.2. Он нужен, потому что в текущем окружении готового include может не быть или он не подключён в проекте напрямую.

В нём объявлены:

- `enum class CameraFacing`
- `enum class CameraCapabilityPriority`
- `enum class PixelFormat`
- `enum class HandleType`
- `enum class CameraParameter`
- `struct CameraCapability`
- `struct CameraCapabilityEx`
- `struct CameraCapabilityRanges`
- `struct PixelFormatDescription`
- `struct CameraInfo`
- `struct YCbCrFrame`
- `struct RawImageFrame`
- `struct Frame`
- `class GraphicBuffer`
- `class CameraListener`
- `class Camera`
- `class CameraManager`
- `extern "C" Aurora::StreamCamera::CameraManager *StreamCameraManager();`

### 2. Заголовок сервиса

Обновлён файл:

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/src/bridgeservice.h`

Добавлен приватный метод:

```cpp
bool tryScreenshotViaStreamCamera(const QString &finalPath, QString *errorMessage);
```

### 3. CMake

Обновлён файл:

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/CMakeLists.txt`

Добавлено:

- `find_library(STREAMCAMERA_LIBRARY NAMES streamcamera)`
- если библиотека найдена, выставляется `AUDB_HAS_STREAMCAMERA=1`
- если библиотека найдена, проект линкуется с `${STREAMCAMERA_LIBRARY}`

### 4. Реализация в bridgeservice.cpp

Обновлён файл:

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/src/bridgeservice.cpp`

Что именно добавлено:

- include `streamcamera_compat.h`
- под `#ifdef AUDB_HAS_STREAMCAMERA` добавлены:
  - выбор лучшей capability
  - конвертация `YCbCr` в `QImage`
  - поворот изображения через `buffer->rotation()`
  - локальный `SingleFrameCameraListener`, который ждёт первый кадр через `std::condition_variable`
- порядок захвата в `screenshot()` изменён на:
  1. `tryScreenshotViaStreamCamera`
  2. `tryScreenshotViaScreenGrab`
  3. старый `screenshotViaDbus`

Реализован `BridgeService::tryScreenshotViaStreamCamera(...)`:

- получает `CameraManager` через `StreamCameraManager()`
- ищет камеру с `info.facing == CameraFacing::Screen`
- запрашивает capability через manager
- выбирает лучшую capability по размеру и fps
- открывает камеру
- ставит listener
- пробует `startCapture` с форматами:
  - `YCbCrFlexible`
  - `YUV420SemiPlanar`
  - `YUV420Planar`
- ждёт до `3000 ms` первый кадр
- останавливает capture
- сохраняет кадр в `finalPath`

При этом старый путь через `ScreenGrab` и старый fallback оставлены на месте.

## Состояние git/worktree

В дереве уже были пользовательские изменения и untracked-файлы. Их не нужно откатывать. В предыдущей сессии это явно учитывалось.

## Что не было полноценно проверено

Хостовая локальная compile-проверка не дала полезного результата из-за проблем вне проекта, связанных с aurora toolchain/spec.

То есть на момент завершения этой сессии код был отредактирован, но сборка и проверка на эмуляторе не были доведены до конца.

## Блокер по сборке в этой сессии

Пользователь собирает через `mb2`, затем подписывает через `rpmsign-external` и отправляет на эмулятор.

В этой агент-сессии прямой вызов SDK не заработал из-за Docker permissions.

Проверенное состояние:

```bash
id
```

Показывало:

```text
uid=1000(kotdath) gid=1000(kotdath) groups=1000(kotdath),10(wheel),968(ollama)
```

То есть в этой сессии отсутствовала группа `docker`.

Прямой вызов:

```bash
docker info
```

давал:

```text
permission denied while trying to connect to the docker API at unix:///var/run/docker.sock
```

И прямой вызов:

```bash
/home/kotdath/AuroraOS/bin/sfdk tools tooling list
```

тоже упирался в тот же сокет.

Важно: у пользователя в его обычном терминале это уже работало, но именно текущая агент-сессия не увидела обновлённые группы. Поэтому для новой сессии это может уже не быть проблемой.

## Что удалось сделать через sg docker -c

Через обход:

```bash
sg docker -c '...'
```

ранее удавалось обращаться к `sfdk` и Docker.

Было подтверждено:

- `sfdk tools target list -a` показывал установленные target:
  - `AuroraOS-5.2.0.180-aarch64`
  - `AuroraOS-5.2.0.180-armv7hl`
  - `AuroraOS-5.2.0.180-x86_64`
- `sfdk engine show` показывал:
  - `dbus.port: 0`
  - `ssh.port: 32222`

Но в новой сессии лучше сначала проверить, заработал ли уже нормальный прямой вызов без `sg`.

## Что нужно сделать в новой сессии

### 1. Проверить доступ к SDK без обхода

Сначала проверить:

```bash
id
docker info
/home/kotdath/AuroraOS/bin/sfdk tools tooling list
```

Если это работает напрямую, продолжать обычным путём.

### 2. Собрать AudbBridge

Нужно дойти до реальной сборки пакета через Aurora SDK/`mb2` или эквивалентный `sfdk` workflow пользователя.

Если потребуется `sfdk`, вероятно имеет смысл проверить target `x86_64`, так как речь об эмуляторе.

### 3. Если сборка упадёт, первым делом проверить streamcamera linkage

Возможный следующий блокер:

файл spec:

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/rpm/ru.kotdath.AudbBridge.spec`

В предыдущей сессии в нём не было явного `BuildRequires` для `streamcamera`.

Если сборка или линковка скажет, что `streamcamera` не найден, нужно:

- проверить наличие dev-пакета в SDK target
- при необходимости добавить нужный `BuildRequires`
- или скорректировать `CMakeLists.txt`, если библиотека должна подтягиваться иначе

### 4. Подписать и поставить на эмулятор

После успешной сборки:

- подписать через `rpmsign-external`
- установить на эмулятор
- вызвать `Ping`
- вызвать `Screenshot`

### 5. Проверить фактический результат

Нужно установить:

- работает ли `StreamCamera` путь
- есть ли ошибки открытия screen camera
- какой `PixelFormat` реально приходит
- корректно ли отрабатывает `mapYCbCr()`
- сохраняется ли итоговая картинка

## Если StreamCamera не заведётся с первого раза

Следующие вероятные места проверки:

- правильно ли объявлены ABI/сигнатуры в `streamcamera_compat.h`
- тот ли namespace и `extern "C"` символ у `StreamCameraManager()`
- совпадают ли сигнатуры `startCapture`, `setListener`, `queryCapabilities`, `openCamera`
- не нужен ли другой overload `startCapture`
- не отличается ли `GraphicBuffer` ABI в реальной библиотеке от документации 5.2

Если будут краши или странные runtime-симптомы без compile error, первым подозреваемым будет именно `streamcamera_compat.h`, потому что это ручная ABI-декларация по документации.

## Критически важные файлы проекта

- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/src/bridgeservice.cpp`
- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/src/bridgeservice.h`
- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/src/streamcamera_compat.h`
- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/CMakeLists.txt`
- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/rpm/ru.kotdath.AudbBridge.spec`
- `/home/kotdath/omp/personal/rust/audb/app/AudbBridge/ru.kotdath.AudbBridge.desktop`

## Краткий handoff

Суть текущего решения:

- простой `ScreenGrab` через чтение `videopipe` как файла не подходит
- в код уже добавлен новый приоритетный путь через `StreamCamera`
- это сделано на основе документации Aurora 5.2 и локального compatibility header
- дальше нужна реальная сборка, установка на эмулятор и проверка, работает ли путь через `CameraFacing::Screen`

