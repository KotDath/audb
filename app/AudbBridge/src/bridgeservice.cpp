#include "bridgeservice.h"

#include "bridgeadaptor.h"

#include <QCoreApplication>
#include <QDebug>
#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusReply>
#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QGuiApplication>
#include <QProcess>
#include <QScreen>
#include <QStandardPaths>

namespace {

const char kServiceName[] = "ru.kotdath.AudbBridge";
const char kObjectPath[] = "/ru/kotdath/AudbBridge";
const char kScreenshotService[] = "org.nemomobile.lipstick";
const char kScreenshotPath[] = "/org/nemomobile/lipstick/screenshot";
const char kScreenshotInterface[] = "org.nemomobile.lipstick";

constexpr int kOrientationPortrait = 1;

}

BridgeService::BridgeService(QObject *parent)
    : QObject(parent)
    , m_executor(QCoreApplication::applicationFilePath())
    , m_statusMessage(QStringLiteral("Bridge is ready."))
{
    new BridgeAdaptor(this);

    QDBusConnection bus = QDBusConnection::sessionBus();
    bus.registerObject(QString::fromLatin1(kObjectPath), this, QDBusConnection::ExportAdaptors);
    bus.registerService(QString::fromLatin1(kServiceName));
    qInfo() << "[AudbBridge] DBus service registered" << kServiceName << kObjectPath;
}

bool BridgeService::hasPassword() const
{
    return m_passwordStore.hasPassword();
}

bool BridgeService::passwordValid() const
{
    return m_passwordStore.passwordValid();
}

QString BridgeService::passwordStatus() const
{
    return effectivePasswordStatus();
}

QString BridgeService::statusMessage() const
{
    return m_statusMessage;
}

QVariantMap BridgeService::statusMap() const
{
    QVariantMap map;
    map.insert(QStringLiteral("service"), QString::fromLatin1(kServiceName));
    map.insert(QStringLiteral("hasPassword"), hasPassword());
    map.insert(QStringLiteral("passwordValid"), passwordValid());
    map.insert(QStringLiteral("passwordStatus"), passwordStatus());
    map.insert(QStringLiteral("statusMessage"), statusMessage());
    map.insert(QStringLiteral("helperBinary"), QCoreApplication::applicationFilePath());
    return map;
}

bool BridgeService::setPasswordForStorage(const QString &password)
{
    qInfo() << "[AudbBridge] validating and storing devel-su password";
    if (password.isEmpty()) {
        setStatusMessage(QStringLiteral("Password cannot be empty."));
        emit stateChanged();
        return false;
    }

    const HelperCommandResult result = m_executor.runHelper({QStringLiteral("--bridge-helper"), QStringLiteral("probe")},
                                                            password);
    if (!result.success) {
        setStatusMessage(QStringLiteral("Password validation failed: %1").arg(result.errorText()));
        emit stateChanged();
        return false;
    }

    m_passwordStore.storePassword(password);
    setStatusMessage(QStringLiteral("devel-su password stored and validated against input backend."));
    emit stateChanged();
    return true;
}

void BridgeService::clearStoredPassword()
{
    m_passwordStore.clear();
    setStatusMessage(QStringLiteral("Stored devel-su password cleared."));
    emit stateChanged();
}

bool BridgeService::selfTest()
{
    qInfo() << "[AudbBridge] running self-test";
    if (!hasPassword()) {
        setStatusMessage(QStringLiteral("No stored devel-su password."));
        emit stateChanged();
        return false;
    }

    const HelperCommandResult result = m_executor.runHelper({QStringLiteral("--bridge-helper"), QStringLiteral("probe")},
                                                            m_passwordStore.password());
    const bool ok = applyHelperResult(result);
    emit stateChanged();
    return ok;
}

bool BridgeService::runTapTest()
{
    const QSize rawSize = currentScreenSize();
    const int orientation = currentOrientation();
    const bool isLandscape = (orientation == 2 || orientation == 8);
    const QSize visibleSize = isLandscape
            ? QSize(rawSize.height(), rawSize.width())
            : rawSize;

    qInfo() << "[AudbBridge] runTapTest"
            << "rawSize=" << rawSize
            << "visibleSize=" << visibleSize
            << "orientation=" << orientation;

    return tap(visibleSize.width() / 2, visibleSize.height() / 2, {});
}

bool BridgeService::runSwipeUpTest()
{
    qInfo() << "[AudbBridge] runSwipeUpTest";
    return swipeDirection(QStringLiteral("du"), {});
}

bool BridgeService::tap(int x, int y, const QVariantMap &options)
{
    const int orientation = options.value(QStringLiteral("noRotate")).toBool() ? kOrientationPortrait : currentOrientation();
    const QSize screenSize = currentScreenSize();
    qInfo() << "[AudbBridge] tap request"
            << "x=" << x
            << "y=" << y
            << "orientation=" << orientation
            << "screenSize=" << screenSize
            << "options=" << options;

    QStringList arguments{
        QStringLiteral("--bridge-helper"), QStringLiteral("tap"),
        QStringLiteral("--x"), QString::number(x),
        QStringLiteral("--y"), QString::number(y),
        QStringLiteral("--orientation"), QString::number(orientation),
        QStringLiteral("--xmax"), QString::number(screenSize.width()),
        QStringLiteral("--ymax"), QString::number(screenSize.height())
    };

    const QString eventDevice = options.value(QStringLiteral("eventDevice")).toString();
    if (!eventDevice.isEmpty()) {
        arguments << QStringLiteral("--event-device") << eventDevice;
    }
    if (options.contains(QStringLiteral("durationMs"))) {
        arguments << QStringLiteral("--duration-ms") << QString::number(options.value(QStringLiteral("durationMs")).toInt());
    }

    return runPrivilegedHelper(arguments);
}

bool BridgeService::swipe(int x1, int y1, int x2, int y2, const QVariantMap &options)
{
    const int orientation = options.value(QStringLiteral("noRotate")).toBool() ? kOrientationPortrait : currentOrientation();
    const QSize screenSize = currentScreenSize();
    qInfo() << "[AudbBridge] swipe request"
            << "x1=" << x1
            << "y1=" << y1
            << "x2=" << x2
            << "y2=" << y2
            << "orientation=" << orientation
            << "screenSize=" << screenSize
            << "options=" << options;

    QStringList arguments{
        QStringLiteral("--bridge-helper"), QStringLiteral("swipe"),
        QStringLiteral("--x1"), QString::number(x1),
        QStringLiteral("--y1"), QString::number(y1),
        QStringLiteral("--x2"), QString::number(x2),
        QStringLiteral("--y2"), QString::number(y2),
        QStringLiteral("--orientation"), QString::number(orientation),
        QStringLiteral("--xmax"), QString::number(screenSize.width()),
        QStringLiteral("--ymax"), QString::number(screenSize.height())
    };

    const QString eventDevice = options.value(QStringLiteral("eventDevice")).toString();
    if (!eventDevice.isEmpty()) {
        arguments << QStringLiteral("--event-device") << eventDevice;
    }
    if (options.contains(QStringLiteral("steps"))) {
        arguments << QStringLiteral("--steps") << QString::number(options.value(QStringLiteral("steps")).toInt());
    }
    if (options.contains(QStringLiteral("stepDelayMs"))) {
        arguments << QStringLiteral("--step-delay-ms") << QString::number(options.value(QStringLiteral("stepDelayMs")).toInt());
    }

    return runPrivilegedHelper(arguments);
}

bool BridgeService::swipeDirection(const QString &direction, const QVariantMap &options)
{
    const int orientation = options.value(QStringLiteral("noRotate")).toBool() ? kOrientationPortrait : currentOrientation();
    const QSize screenSize = currentScreenSize();
    qInfo() << "[AudbBridge] swipeDirection request"
            << "direction=" << direction
            << "orientation=" << orientation
            << "screenSize=" << screenSize
            << "options=" << options;

    QStringList arguments{
        QStringLiteral("--bridge-helper"), QStringLiteral("swipe"),
        QStringLiteral("--direction"), direction,
        QStringLiteral("--orientation"), QString::number(orientation),
        QStringLiteral("--xmax"), QString::number(screenSize.width()),
        QStringLiteral("--ymax"), QString::number(screenSize.height())
    };

    const QString eventDevice = options.value(QStringLiteral("eventDevice")).toString();
    if (!eventDevice.isEmpty()) {
        arguments << QStringLiteral("--event-device") << eventDevice;
    }

    return runPrivilegedHelper(arguments);
}

bool BridgeService::key(const QString &keyName)
{
    const int orientation = currentOrientation();
    const QSize screenSize = currentScreenSize();
    qInfo() << "[AudbBridge] key request"
            << "key=" << keyName
            << "orientation=" << orientation
            << "screenSize=" << screenSize;

    QStringList arguments{
        QStringLiteral("--bridge-helper"), QStringLiteral("key"),
        QStringLiteral("--key"), keyName,
        QStringLiteral("--orientation"), QString::number(orientation),
        QStringLiteral("--xmax"), QString::number(screenSize.width()),
        QStringLiteral("--ymax"), QString::number(screenSize.height())
    };
    return runPrivilegedHelper(arguments);
}

QString BridgeService::screenshot(const QString &outputPath)
{
    const QString finalPath = outputPath.trimmed().isEmpty() ? defaultScreenshotPath() : outputPath;

    QDir().mkpath(QFileInfo(finalPath).absolutePath());

    QDBusInterface iface(QString::fromLatin1(kScreenshotService),
                         QString::fromLatin1(kScreenshotPath),
                         QString::fromLatin1(kScreenshotInterface),
                         QDBusConnection::sessionBus());
    if (!iface.isValid()) {
        setStatusMessage(QStringLiteral("Screenshot service is not available on the session bus."));
        emit stateChanged();
        return QString();
    }

    const QDBusReply<void> reply = iface.call(QStringLiteral("saveScreenshot"), finalPath);
    if (!reply.isValid()) {
        setStatusMessage(QStringLiteral("Screenshot failed: %1").arg(reply.error().message().trimmed()));
        emit stateChanged();
        return QString();
    }

    setStatusMessage(QStringLiteral("Screenshot saved to %1").arg(finalPath));
    emit stateChanged();
    return finalPath;
}

QSize BridgeService::currentScreenSize() const
{
    if (QScreen *screen = QGuiApplication::primaryScreen()) {
        const QSize size = screen->size();
        if (size.isValid() && !size.isEmpty()) {
            return size;
        }
    }

    return QSize(720, 1440);
}

int BridgeService::currentOrientation() const
{
    QProcess process;
    process.start(QStringLiteral("dconf"),
                  {QStringLiteral("read"), QStringLiteral("/desktop/lipstick-jolla-home/dialog_orientation")});
    if (!process.waitForFinished(2000)) {
        process.kill();
        process.waitForFinished();
        return kOrientationPortrait;
    }

    bool ok = false;
    const int orientation = QString::fromUtf8(process.readAllStandardOutput()).trimmed().toInt(&ok);
    qInfo() << "[AudbBridge] currentOrientation" << (ok ? orientation : kOrientationPortrait);
    return ok ? orientation : kOrientationPortrait;
}

QString BridgeService::effectivePasswordStatus() const
{
    if (!hasPassword()) {
        return QStringLiteral("password not set");
    }
    if (!passwordValid()) {
        return QStringLiteral("last authentication failed");
    }
    return QStringLiteral("password stored");
}

bool BridgeService::runPrivilegedHelper(const QStringList &arguments)
{
    if (!hasPassword()) {
        setStatusMessage(QStringLiteral("No stored devel-su password."));
        emit stateChanged();
        return false;
    }

    const HelperCommandResult result = m_executor.runHelper(arguments, m_passwordStore.password());
    const bool ok = applyHelperResult(result);
    emit stateChanged();
    return ok;
}

QString BridgeService::defaultScreenshotPath() const
{
    QString pictures = QStandardPaths::writableLocation(QStandardPaths::PicturesLocation);
    if (pictures.isEmpty()) {
        pictures = QDir::homePath() + QStringLiteral("/Pictures");
    }

    const QString directory = QDir(pictures).absoluteFilePath(QStringLiteral("Screenshots"));
    const QString fileName = QStringLiteral("audbbridge_%1.png")
            .arg(QDateTime::currentDateTime().toString(QStringLiteral("yyyyMMdd_hhmmss")));
    return QDir(directory).absoluteFilePath(fileName);
}

void BridgeService::setStatusMessage(const QString &message)
{
    m_statusMessage = message;
}

bool BridgeService::applyHelperResult(const HelperCommandResult &result)
{
    if (result.success) {
        m_passwordStore.markPasswordValid();
        setStatusMessage(result.stdOut.trimmed().isEmpty()
                         ? QStringLiteral("Operation completed successfully.")
                         : result.stdOut.trimmed());
        qInfo() << "[AudbBridge] helper result success" << statusMessage();
        return true;
    }

    if (result.authFailure()) {
        m_passwordStore.markPasswordInvalid();
        setStatusMessage(QStringLiteral("Stored devel-su password is invalid. Please set it again."));
        qWarning() << "[AudbBridge] helper auth failure";
        return false;
    }

    setStatusMessage(result.errorText());
    qWarning() << "[AudbBridge] helper result failure" << result.errorText();
    return false;
}
