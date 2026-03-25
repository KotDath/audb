#include "bridgeadaptor.h"

#include "bridgeservice.h"

BridgeAdaptor::BridgeAdaptor(BridgeService *service)
    : QDBusAbstractAdaptor(service)
    , m_service(service)
{
}

bool BridgeAdaptor::Ping() const
{
    return true;
}

bool BridgeAdaptor::HasPassword() const
{
    return m_service->hasPassword();
}

QVariantMap BridgeAdaptor::GetStatus() const
{
    return m_service->statusMap();
}

bool BridgeAdaptor::Tap(int x, int y, const QVariantMap &options)
{
    return m_service->tap(x, y, options);
}

bool BridgeAdaptor::Swipe(int x1, int y1, int x2, int y2, const QVariantMap &options)
{
    return m_service->swipe(x1, y1, x2, y2, options);
}

bool BridgeAdaptor::SwipeDirection(const QString &direction, const QVariantMap &options)
{
    return m_service->swipeDirection(direction, options);
}

bool BridgeAdaptor::Key(const QString &keyName)
{
    return m_service->key(keyName);
}

QString BridgeAdaptor::Screenshot(const QString &outputPath)
{
    return m_service->screenshot(outputPath);
}
