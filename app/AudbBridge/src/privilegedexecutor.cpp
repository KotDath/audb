#include "privilegedexecutor.h"

#include <QDebug>
#include <QProcess>

namespace {

constexpr int kProcessTimeoutMs = 15000;

}

bool HelperCommandResult::authFailure() const
{
    const QString combined = (stdOut + QLatin1Char('\n') + stdErr).toLower();
    return combined.contains(QStringLiteral("authentication failure"))
            || combined.contains(QStringLiteral("incorrect password"))
            || combined.contains(QStringLiteral("wrong password"))
            || combined.contains(QStringLiteral("password: sorry"))
            || combined.contains(QStringLiteral("devel-su: authentication failed"))
            || combined.contains(QStringLiteral("devel-su: sorry"));
}

QString HelperCommandResult::errorText() const
{
    if (!stdErr.trimmed().isEmpty()) {
        return stdErr.trimmed();
    }
    if (!stdOut.trimmed().isEmpty()) {
        return stdOut.trimmed();
    }
    return QStringLiteral("devel-su helper failed");
}

PrivilegedExecutor::PrivilegedExecutor(QString helperBinaryPath)
    : m_helperBinaryPath(std::move(helperBinaryPath))
{
}

HelperCommandResult PrivilegedExecutor::runHelper(const QStringList &helperArguments,
                                                  const QString &password) const
{
    HelperCommandResult result;
    if (password.isEmpty()) {
        result.stdErr = QStringLiteral("No stored devel-su password.");
        return result;
    }

    QStringList quotedParts;
    quotedParts << shellQuote(m_helperBinaryPath);
    for (const QString &argument : helperArguments) {
        quotedParts << shellQuote(argument);
    }

    qInfo().noquote() << "[AudbBridge] runHelper start"
                      << "binary=" << m_helperBinaryPath
                      << "args=" << helperArguments.join(QLatin1Char(' '));

    QProcess process;
    process.setProgram(QStringLiteral("devel-su"));
    process.setArguments({QStringLiteral("sh"), QStringLiteral("-c"), quotedParts.join(QLatin1Char(' '))});
    process.setProcessChannelMode(QProcess::SeparateChannels);
    process.start();
    if (!process.waitForStarted(kProcessTimeoutMs)) {
        result.stdErr = QStringLiteral("Failed to start devel-su.");
        return result;
    }

    process.write(password.toUtf8());
    process.write("\n");
    process.closeWriteChannel();

    if (!process.waitForFinished(kProcessTimeoutMs)) {
        process.kill();
        process.waitForFinished();
        result.stdErr = QStringLiteral("Timed out waiting for devel-su helper.");
        return result;
    }

    result.exitCode = process.exitCode();
    result.stdOut = QString::fromUtf8(process.readAllStandardOutput());
    result.stdErr = QString::fromUtf8(process.readAllStandardError());
    result.success = (process.exitStatus() == QProcess::NormalExit && process.exitCode() == 0);

    qInfo().noquote() << "[AudbBridge] runHelper finished"
                      << "exitCode=" << result.exitCode
                      << "success=" << result.success;
    if (!result.stdOut.trimmed().isEmpty()) {
        qInfo().noquote() << "[AudbBridge] helper stdout:" << result.stdOut.trimmed();
    }
    if (!result.stdErr.trimmed().isEmpty()) {
        qWarning().noquote() << "[AudbBridge] helper stderr:" << result.stdErr.trimmed();
    }

    return result;
}

QString PrivilegedExecutor::shellQuote(const QString &value)
{
    QString quoted = value;
    quoted.replace(QLatin1Char('\''), QStringLiteral("'\"'\"'"));
    return QStringLiteral("'%1'").arg(quoted);
}
