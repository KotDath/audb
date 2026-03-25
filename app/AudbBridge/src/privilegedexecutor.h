#pragma once

#include <QString>
#include <QStringList>

struct HelperCommandResult
{
    bool success = false;
    int exitCode = -1;
    QString stdOut;
    QString stdErr;

    bool authFailure() const;
    QString errorText() const;
};

class PrivilegedExecutor
{
public:
    explicit PrivilegedExecutor(QString helperBinaryPath);

    HelperCommandResult runHelper(const QStringList &helperArguments,
                                  const QString &password) const;

private:
    static QString shellQuote(const QString &value);

    QString m_helperBinaryPath;
};
