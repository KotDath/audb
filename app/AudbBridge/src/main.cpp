#include "bridgeservice.h"
#include "helpercli.h"

#include <auroraapp.h>
#include <QCoreApplication>
#include <QStringList>
#include <QtQuick>

int main(int argc, char *argv[])
{
    QStringList arguments;
    arguments.reserve(argc);
    for (int index = 0; index < argc; ++index) {
        arguments.append(QString::fromLocal8Bit(argv[index]));
    }

    if (arguments.contains(QStringLiteral("--bridge-helper"))) {
        QCoreApplication application(argc, argv);
        application.setOrganizationName(QStringLiteral("ru.kotdath"));
        application.setApplicationName(QStringLiteral("AudbBridge"));
        return HelperCli::run(application.arguments());
    }

    QScopedPointer<QGuiApplication> application(Aurora::Application::application(argc, argv));
    application->setOrganizationName(QStringLiteral("ru.kotdath"));
    application->setApplicationName(QStringLiteral("AudbBridge"));

    QScopedPointer<QQuickView> view(Aurora::Application::createView());
    BridgeService bridgeService(view.data());
    view->rootContext()->setContextProperty(QStringLiteral("bridgeService"), &bridgeService);
    view->setSource(Aurora::Application::pathTo(QStringLiteral("qml/AudbBridge.qml")));
    view->show();

    return application->exec();
}
