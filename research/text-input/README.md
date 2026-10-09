# Ввод текста и клавиш: проверка на Авроре

Дата: 2026-10-08. Устройство `phone` — KVADRA_T, Aurora 5.2.0.259,
aarch64, defaultuser. Рабочий системный пакет: `audb-agent-0.3.0-10`.
CLI/daemon protocol 8, agent protocol 3, внутренний Maliit input bridge 1.

## Механизм

На устройстве установлены `maliit-framework-wayland-0.99.1+git18`,
`jolla-keyboard-0.14.9`, Qt 5 и английская/русская Presage-клавиатуры.
`org.maliit.server` на пользовательском D-Bus предоставляет только адрес
приватного соединения. `com.jolla.keyboard` не предоставляет API commit.
Публичного D-Bus вызова для произвольного текста не обнаружено.

В Maliit QML-plugin получает `MInputMethodQuick` через root QQmlContext.
Штатный InputHandler вызывает `sendCommit(text)`. Собственный plugin host
неактивен и не может сам отправлять commit: MInputMethodHost проверяет enabled.
Поэтому фоновый `zz-audb-input.qml` загружается вместе со штатной клавиатурой,
а C++ QML extension обнаруживает **её активный** InputMethodQuick через
QGuiApplication::allWindows, QQuickView::rootObject и QQmlEngine::contextForObject.
Проверяет meta-object API и вызывает sendCommit(QString,int,int,int).
Отдельного окна приложения и изменения keyboard/layout configuration нет.

Это проверенная интеграция с внутренним QML-контекстом Maliit в Авроре 5.2;
public Qt APIs позволяют обходиться без private headers, но совместимость
с другим Maliit всё равно требует проверки. В отсутствие активного совместимого
контекста команда возвращает INPUT_NOT_FOCUSED и ничего не отправляет.

Native sources: `audb-agent/native/input/`. Собираются SDK g++/moc со
строгими предупреждениями и Qt5Quick/Qt5Qml/Qt5Network. Производственная
служба и agentctl написаны на Rust; Python применяется только на хосте при
сборке/исследовании. Подключение к локальному IME-сокету выполняет agentctl
с обычным пользовательским UID, root-служба не читает содержимое редактора.

Клавиши — отдельное постоянное `/dev/uinput` устройство `audb-keyboard`.
Пользовательский JSON проходит в root-службу через прежний root:input socket.
Обрабатываются только перечисленные имена; произвольного evdev-кода,
команд shell или IPC-вызова в API нет. Каждая операция — press/release.

## Проверенные результаты

Артефакты находятся в `target/text-evidence/`:

- `qt-unicode.png`: русская строка, English, emoji при EN-клавиатуре;
- `qt-keys.png`: Backspace и Left, затем вставка в середину;
- `flutter-unicode-verified.json`: точное совпадение строки
  `Привет, Aurora! 🦀 é 中文 $"\\`, newline, `Вторая строка`, tab и `END`;
- `flutter-keys-verified.json`: Backspace, Left, Delete, Enter и вставка Ж;
- `flutter-paced-verified.json`: точное сравнение посимвольного ввода с emoji
  и decomposed e + combining accent при delay 30 ms;
- `flutter-russian-layout-verified.json`: English/кириллица при RU-клавиатуре;
- `flutter-focus-cancel-verified.json`: введён только префикс `0123`, второе
  поле пустое, OUTCOME_UNKNOWN, параллельный запрос отклонён INPUT_BUSY;
- `flutter-client-cancel-verified.json`: SIGTERM agentctl прекращает ввод,
  после дополнительной секунды состояние редактора не меняется;
- `setup-10.json`: установка, readiness и text/key capabilities;
- `text-tests.log` и `text-clippy.log` в target: 75 тестов, Clippy -D warnings.

Flutter fixture использует Flutter Aurora 3.41.4. Исходник — `probe.dart`.
Каждое изменение controller пишет JSON в приватный home приложения.
Root read-back выполнялся через `/proc/<actual application PID>/root/...`:
обычный host home не содержит этот файл из-за Sailjail private-home.
Проверка не зависит от OCR, UI tree, Clipboard или самоотчёта bridge.

Первая версия быстрой посимвольной отправки воспроизводимо переставляла
пробел возле emoji во Flutter: следующий commit приходил до обновления
курсора. Сейчас default delay 0 отправляет всю строку один раз.
При delay >0 каждый следующий commit ждёт editorStateUpdate, ожидаемые
UTF-16 cursor и surroundingText. Неуспех останавливает остаток; текст
редактора не включается в IPC-ответ, stderr или журнал. Newline-последовательности
объединяются с соседним текстом, потому что Maliit переводит одиночный '\n'
в Return. Newline-only input отвергается; для него есть `key enter`.

Фокус определяется уведомлениями Maliit и проверкой редакторского состояния.
Приложение может переключать внутренние поля без отдельного Maliit focus
notification. Поэтому проверка text/cursor также блокирует продолжение при
несовпадении; различить два внутренних поля с полностью одинаковым состоянием
по одному Maliit API невозможно. Для обычной автоматизации предпочтителен
однократный commit, delay 0, без вмешательства в фокус во время команды.

## Воспроизведение

```bash
cd /home/kotdath/omp/personal/rust/audb
./target/debug/audb --device phone --json status
# Сначала открыть приложение и коснуться нужного текстового поля.
./target/debug/audb --device phone --json text 'Привет, Aurora! 🦀'
./target/debug/audb --device phone --json text --stdin < message.txt
./target/debug/audb --device phone --json key backspace
./target/debug/audb --device phone --json key enter
```

Fixture можно создать командой `flutter create --platforms aurora --org
ru.kotdath --project-name audb_input_probe`, заменить lib/main.dart на probe.dart
и собрать `flutter build aurora --debug --target-platform aurora-arm64`.
Использовался `--sdk-dir /home/kotdath/AuroraOS`, чтобы выбрать установленный SDK.
У generated Flutter RPM уже была Regular development signature; она сохранена.
Regular rpm-validator завершился 0. Временное приложение после проверки удалено.
Прежняя раскладка и исходные grants/showPrompt secure-storage example восстановлены.

Установка системного RPM и setup перезапускают root-службу; setup также
перезапускает пользовательский maliit-server. Приложение, открытое до setup,
может потребовать одного перезапуска для восстановления input context.
Просто копировать новую Qt-библиотеку на устройство недостаточно: Аврора
отклонила mmap/запуск до установки подписанного RPM.

Системный agent подписан Regular developer certificate. Regular validator
завершается 1 из-за system locations/scriptlets/service и прочих правил
профиля обычного приложения. Установка выполнена ранее согласованным system
RPM route с локальным transaction validation override. Trusted root cert для
отдельной проверки цепочки не был предоставлен. Это developer-device package.

Реальная VM выключена. Предпочтение agent, SSH JSON stdin, UTF-8 и отсутствие
повтора после неизвестного результата проверены emulator-интеграционным тестом
с тестовым SSH-сервером и отсутствующим QMP. Аппаратная работа IME/uinput на
эмуляторе и другие версии/модели остаются отдельной проверкой.

Найдено сопутствующее прежнее ограничение package install: бинарный RPM
передаётся daemon JSON-массивом bytes; RPM 33 MiB превышает 100 MiB frame cap
после сериализации. Fixture передан SCP и установлен обычным APM.Install.
Это не блокирует малый системный agent RPM; исправление больших RPM отдельно
от задачи ввода. Ошибка не повторялась автоматически.

## Источники

- [Sailfish Maliit packaging](https://github.com/sailfishos/maliit-framework),
  исследован upstream submodule commit ba6f7eda338a913f2c339eada3f0382e04f7dd67;
- [Maliit input method host](https://github.com/maliit/framework/blob/master/src/minputmethodhost.cpp);
- [Qt QQmlEngine::contextForObject](https://doc.qt.io/archives/qt-5.15/qqmlengine.html#contextForObject);
- [Qt QGuiApplication::allWindows](https://doc.qt.io/archives/qt-5.15/qguiapplication.html#allWindows).

Фактические Aurora QML-файлы и D-Bus introspection проверены на самом устройстве;
upstream source объясняет гипотезу, а device JSON read-back подтверждает результат.
