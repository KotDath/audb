#pragma once

#include "passwordstore.h"
#include "privilegedexecutor.h"

#include <QObject>
#include <QQmlEngine>
#include <QSize>
#include <QVariantMap>

class BridgeService : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool hasPassword READ hasPassword NOTIFY stateChanged)
    Q_PROPERTY(bool passwordValid READ passwordValid NOTIFY stateChanged)
    Q_PROPERTY(QString passwordStatus READ passwordStatus NOTIFY stateChanged)
    Q_PROPERTY(QString statusMessage READ statusMessage NOTIFY stateChanged)

public:
    explicit BridgeService(QObject *parent = nullptr);

    bool hasPassword() const;
    bool passwordValid() const;
    QString passwordStatus() const;
    QString statusMessage() const;
    QVariantMap statusMap() const;

    Q_INVOKABLE bool setPasswordForStorage(const QString &password);
    Q_INVOKABLE void clearStoredPassword();
    Q_INVOKABLE bool selfTest();
    Q_INVOKABLE bool runTapTest();
    Q_INVOKABLE bool runSwipeUpTest();
    Q_INVOKABLE void setScreenshotGrabber(QObject *grabber);

    bool tap(int x, int y, const QVariantMap &options);
    bool swipe(int x1, int y1, int x2, int y2, const QVariantMap &options);
    bool swipeDirection(const QString &direction, const QVariantMap &options);
    bool key(const QString &keyName);
    bool setClipboardText(const QString &text);
    QString clipboardText() const;
    Q_INVOKABLE QString screenshot(const QString &outputPath);

signals:
    void stateChanged();

private:
    QSize currentScreenSize() const;
    int currentOrientation() const;
    QString effectivePasswordStatus() const;
    bool runPrivilegedHelper(const QStringList &arguments);
    bool tryScreenshotViaStreamCamera(const QString &finalPath, QString *errorMessage);
    bool tryScreenshotViaQml(const QString &finalPath, QString *errorMessage);
    bool tryScreenshotViaScreenGrab(const QString &finalPath, QString *errorMessage);
    QString screenshotViaDbus(const QString &finalPath);
    QString defaultScreenshotPath() const;
    void setStatusMessage(const QString &message);
    bool applyHelperResult(const HelperCommandResult &result);

    PasswordStore m_passwordStore;
    PrivilegedExecutor m_executor;
    QQmlEngine *m_qmlEngine = nullptr;
    QObject *m_screenshotGrabber = nullptr;
    QString m_statusMessage;
};
