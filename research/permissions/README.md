# Разрешения приложений Aurora и их автоматизация через audb

На Aurora 5.2.0.259 управление разрешениями доступно через системный D-Bus Sailjail. Для точечной выдачи требуется сначала отключить диалог, затем записать требуемый набор: переключение состояния диалога само меняет grants. Эти особенности определяют порядок операций в реализации audb.

## API и права доступа

Сервис и интерфейс: `org.sailfishos.sailjaild1`, путь: `/org/sailfishos/sailjaild1`, шина: system.

| Метод | Вход | Результат |
| --- | --- | --- |
| GetApplications | — | Список application ID |
| GetAppInfo | application ID | Словарь, включая Id, Mode, Exec и Permissions |
| GetGrantedPermissions | uint32 UID, application ID | Сохранённый набор grants |
| SetGrantedPermissions | uint32 UID, application ID, массив строк | Замена набора grants |
| GetShowPrompt | uint32 UID, application ID | int32, 0 или 1 |
| SetShowPrompt | uint32 UID, application ID, int32 | Переключение диалога и изменение grants |
| QueryLaunchPermissions | application ID | Разрешения запуска для активной сессии либо ошибка |
| PromptLaunchPermissions | application ID | Разрешения запуска, с возможным ожиданием ответа на диалог |

Чтение GetAppInfo/GetGrantedPermissions/GetShowPrompt доступно обычному defaultuser. Оба setter возвращают AccessDenied для него; root-вызовы проходят. Системная служба audb подходит для этих операций. UID следует брать из SO_PEERCRED, а не фиксировать 100000. System D-Bus не требует подключения к пользовательской шине или чтения файлов home службой audb.

Application ID соответствует зарегистрированному Sailjail desktop ID. RPM Name, Exec и OrganizationName/ApplicationName не заменяют этот идентификатор. Например, `jolla-settings` является допустимым ID без точки; текущий app::validate_package с требованием точки для нового permission API не подходит. Неизвестный ID возвращает InvalidArgs. Разрешать ID следует по GetApplications/GetAppInfo, а значения передавать типизированными D-Bus аргументами.

## Проверенное поведение на устройстве

Устройство: KVADRA_T, 192.168.2.44; sailjail-daemon 1.3.35-1.12.1.omp. Тестовое приложение: `ru.aurora.flutter_secure_storage_example`, объявляет UserDirs, DeviceInfo и RemovableMedia.

| Действие | ShowPrompt после действия | Grants после действия |
| --- | --- | --- |
| Исходное состояние | 1 | Пусто |
| SetGrantedPermissions с UserDirs при prompt 1 | 1 | Пусто, несмотря на успешный ответ |
| Переключить prompt из 1 в 0 | 0 | Все три объявленных разрешения |
| Записать UserDirs при prompt 0 | 0 | Только UserDirs |
| Повторить prompt 0 при prompt 0 | 0 | UserDirs сохраняется |
| Записать DeviceInfo при prompt 0 | 0 | Только DeviceInfo, UserDirs удалён |
| Записать UserDirs и DeviceInfo | 0 | Оба разрешения |
| Записать DeviceInfo и необъявленное Audio | 0 | Только DeviceInfo, Audio отфильтрован |
| Записать пустой набор при prompt 0 | 0 | Пусто |
| Переключить prompt из 0 в 1 | 1 | Пусто, даже если до этого были выданы все разрешения |

При prompt 0 QueryLaunchPermissions и PromptLaunchPermissions возвращают grants без ожидания диалога; вызов PromptLaunchPermissions с полным набором завершился за 0.28 секунды. При prompt 1 QueryLaunchPermissions возвращает AuthFailed: Not allowed. Пустой набор с prompt 0 возвращается как пустой массив. Доступ приложения к конкретным ресурсам и визуальный цикл реального запуска должны проверяться следующим этапом, отдельно от успешного D-Bus ответа.

Значения 2 и 99 принимаются setter без ошибки и читаются как 1. Будущий интерфейс должен принимать только явные enable/disable. Отрицательное значение в предварительном тесте отклонено самим CLI gdbus как опция, поэтому семантика отрицательных значений ОС не установлена.

После экспериментов исходные grants, prompt и запись приложения восстановлены. Эмуляторный SSH на 127.0.0.1:2223 недоступен (Connection refused); эти результаты относятся к телефону, а не ко всем версиям Aurora.

## Хранение состояния

Объявленные разрешения берутся из desktop metadata: на этом приложении секция `[X-Application]` содержит `Permissions=UserDirs;DeviceInfo;RemovableMedia`. GetAppInfo возвращает обработанный список и остаётся основным источником для audb.

Профили разрешений находятся в `/etc/sailjail/permissions/*.permission`, конфигурация — в `/etc/sailjail/config/*.conf`. Сохранённые настройки пользователя на этом устройстве лежат в `/home/.system/var/lib/sailjail/settings/user-<UID>.settings`. Каталог root 0750, файл root 0640. Запись приложения имеет вид:

```ini
[ru.aurora.flutter_secure_storage_example]
Version=2
Prompt=1
Agreed=0
Autogrant=0
Granted=
Permissions=UserDirs;DeviceInfo;RemovableMedia;
```

audb должен изменять настройки через D-Bus и проверять их чтением. Ручная правка файла обходит состояние работающего daemon. Grants и сведения об автоматических разрешениях/ограничениях ОС следует показывать отдельно: наличие GetAlwaysAllowedApplications и глобальных allow/deny механизмов не делает grants полным описанием эффективного доступа.

## Реализация audb

`permission list`, `grant`, `revoke`, `reset` и `prompt` реализованы. [Примеры CLI и контракт](../../README.md#application-permissions) описывают использование. List возвращает applicationId, UID, declared, granted, boolean showPrompt, mode и optional alwaysAllowed. Permission действия не зависят от Qt, DRM или uinput. Служба может работать при недоступном input backend; запросы имеют optional display.

Точечная команда: `permission grant APP UserDirs --disable-prompt`. Если prompt включён и флаг отсутствует, возвращается PROMPT_ENABLED до изменений. С флагом служба читает before, отключает prompt, записывает объединение исходных grants с выбранными объявленными разрешениями и проверяет after. `--all-requested` получает набор из GetAppInfo. При уже отключённом prompt обычный grant объединяет набор, а revoke удаляет выбранные элементы. Необъявленное разрешение отклоняется до setter.

`prompt --disable` сохраняет исходные grants, переписывая их после переключения в 0. `prompt --enable` и `reset` устанавливают включённый диалог и пустой пользовательский набор. Данные приложения не очищаются. Операции не меняют глобальные allow/deny политики.

Служба сериализует операции и использует типизированный zbus D-Bus, проверяя Aurora сигнатуры. UID поступает от SO_PEERCRED и не принимается из JSON клиента. Worker ограничен 20 секундами, agentctl ожидает ответ до 25 секунд; SSH/CLI timeout не гарантирует немедленной отмены. Ответ с неизвестным результатом не повторяет запись. Неатомарная последовательность setter возвращает phase и фактическое before/after при ошибке, когда read-back доступен. Read-back несовпадение — PERMISSION_VERIFY_FAILED. Протоколы CLI/daemon и agent обновлены до 7 и 2 соответственно.

На телефоне установлен подписанный `audb-agent-0.3.0-7.aarch64.rpm`. Проверены все CLI операции, идемпотентность и машинные ошибки. List дополнительно проверен при неверных Wayland/Qt настройках. После выдачи всех разрешений Flutter secure-storage example дошёл до UI без диалога; затем остановлен, исходные permissions и prompt восстановлены. Скриншот — target/permission-evidence/launched-settled.png, протоколы — audb-verification.json, launch-verification.json и setup.json. Все 70 workspace тестов и Clippy прошли. Emulator SSH интеграционный тест подтверждает передачу permission JSON без QMP и root SSH; реальные OS методы эмулятора пока не проверены.

## Источники

[CI реализация _set_permissions](/home/kotdath/omp/work/flutter/gitlab-ci-configuration/scripts/jenkins-deploy-flutter.py:160) получает Permissions, записывает весь массив и вызывает SetShowPrompt(..., 0). Она использует фиксированный UID 100000 и текстовый парсер dbus-send; эти части не подходят для общего API audb. На проверенном исходном состоянии запись grants до выключения prompt не действует, а финальное переключение само выдаёт весь набор. Поэтому CI последовательность нельзя переносить без проверки результата.

[Upstream Sailjail daemon](https://github.com/sailfishos/sailjail/blob/b64134d64a937c37d7d65cd8fad560013a8e783a/daemon/README.md) описывает GetLaunchAllowed/SetLaunchAllowed с enum UNSET/ALWAYS/NEVER. Aurora экспортирует GetShowPrompt/SetShowPrompt с другим поведением. Upstream используется для понимания архитектуры; точная Aurora семантика подтверждена на устройстве.

Протоколы проверок: `target/permission-evidence/app-info.json`, `baseline.json`, `dbus-experiment-preliminary.json`, `dbus-experiment.json` и `dbus-edge-cases.json`. Они содержат ответы D-Bus, отказ записи обычному пользователю и подтверждение восстановления; паролей в них нет.
