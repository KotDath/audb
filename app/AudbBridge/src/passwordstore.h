#pragma once

#include <QString>

class PasswordStore
{
public:
    PasswordStore();

    bool hasPassword() const;
    QString password() const;
    bool passwordValid() const;

    void storePassword(const QString &password);
    void markPasswordValid();
    void markPasswordInvalid();
    void clear();

private:
    QString settingsGroup() const;
};
