#include <QGuiApplication>
#include <QScreen>
#include <QJsonDocument>
#include <QJsonObject>
#include <cstdio>
int main(int argc, char **argv) {
    QGuiApplication app(argc, argv);
    QScreen *screen = app.primaryScreen();
    if (!screen) return 1;
    screen->setOrientationUpdateMask(Qt::PortraitOrientation | Qt::LandscapeOrientation |
                                    Qt::InvertedPortraitOrientation | Qt::InvertedLandscapeOrientation);
    app.processEvents();
    auto native = screen->nativeOrientation();
    auto orientation = screen->orientation();
    if (orientation == Qt::PrimaryOrientation) orientation = native;
    // Inverse transform: screenshot coordinates -> native input coordinates.
    const int rotation = screen->angleBetween(orientation, native);
    const QSize size = screen->size();
    // On Aurora Wayland screen size describes the native panel; orientation is separate.
    QJsonObject result{{"nativeWidth",size.width()},{"nativeHeight",size.height()},
                       {"rotation",rotation}};
    const QByteArray bytes = QJsonDocument(result).toJson(QJsonDocument::Compact);
    std::printf("%s\n",bytes.constData());
    return 0;
}
