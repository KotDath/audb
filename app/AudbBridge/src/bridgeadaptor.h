#pragma once

#include <QDBusAbstractAdaptor>
#include <QVariantMap>

class BridgeService;

class BridgeAdaptor : public QDBusAbstractAdaptor
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "ru.kotdath.AudbBridge")

public:
    explicit BridgeAdaptor(BridgeService *service);

public slots:
    bool Ping() const;
    bool HasPassword() const;
    QVariantMap GetStatus() const;
    bool Tap(int x, int y, const QVariantMap &options);
    bool Swipe(int x1, int y1, int x2, int y2, const QVariantMap &options);
    bool SwipeDirection(const QString &direction, const QVariantMap &options);
    bool Key(const QString &keyName);
    bool SetClipboardText(const QString &text);
    QString GetClipboardText() const;
    QString Screenshot(const QString &outputPath);

private:
    BridgeService *m_service;
};
