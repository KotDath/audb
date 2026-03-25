#include "passwordstore.h"

#include <QSettings>

namespace {

const char kSettingsGroup[] = "auth";
const char kPasswordKey[] = "password";
const char kPasswordValidKey[] = "passwordValid";

}

PasswordStore::PasswordStore() = default;

bool PasswordStore::hasPassword() const
{
    QSettings settings;
    settings.beginGroup(settingsGroup());
    const QString value = settings.value(QLatin1String(kPasswordKey)).toString();
    settings.endGroup();
    return !value.isEmpty();
}

QString PasswordStore::password() const
{
    QSettings settings;
    settings.beginGroup(settingsGroup());
    const QString value = settings.value(QLatin1String(kPasswordKey)).toString();
    settings.endGroup();
    return value;
}

bool PasswordStore::passwordValid() const
{
    QSettings settings;
    settings.beginGroup(settingsGroup());
    const bool value = settings.value(QLatin1String(kPasswordValidKey), false).toBool();
    settings.endGroup();
    return hasPassword() && value;
}

void PasswordStore::storePassword(const QString &password)
{
    QSettings settings;
    settings.beginGroup(settingsGroup());
    settings.setValue(QLatin1String(kPasswordKey), password);
    settings.setValue(QLatin1String(kPasswordValidKey), true);
    settings.endGroup();
    settings.sync();
}

void PasswordStore::markPasswordValid()
{
    if (!hasPassword()) {
        return;
    }

    QSettings settings;
    settings.beginGroup(settingsGroup());
    settings.setValue(QLatin1String(kPasswordValidKey), true);
    settings.endGroup();
    settings.sync();
}

void PasswordStore::markPasswordInvalid()
{
    if (!hasPassword()) {
        return;
    }

    QSettings settings;
    settings.beginGroup(settingsGroup());
    settings.setValue(QLatin1String(kPasswordValidKey), false);
    settings.endGroup();
    settings.sync();
}

void PasswordStore::clear()
{
    QSettings settings;
    settings.beginGroup(settingsGroup());
    settings.remove(QLatin1String(kPasswordKey));
    settings.remove(QLatin1String(kPasswordValidKey));
    settings.endGroup();
    settings.sync();
}

QString PasswordStore::settingsGroup() const
{
    return QLatin1String(kSettingsGroup);
}
