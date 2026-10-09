// SPDX-License-Identifier: MIT
// Background Maliit plugin. The stock keyboard, layouts and settings stay active.
#include <QQmlExtensionPlugin>
#include <QQmlEngine>
#include <QQmlContext>
#include <QGuiApplication>
#include <QQuickView>
#include <QQuickItem>
#include <QLocalServer>
#include <QLocalSocket>
#include <QJsonDocument>
#include <QJsonObject>
#include <QPointer>
#include <QSet>
#include <QTimer>
#include <QElapsedTimer>
#include <sys/socket.h>
#include <unistd.h>

class InputBridge : public QObject {
    Q_OBJECT
    QLocalServer server;
    QPointer<QLocalSocket> pending;
    QPointer<QObject> target;
    QTimer timer;
    QStringList chunks;
    int submitted = 0;
    int total = 0;
    int connections = 0;
    bool focusChanged = false;
    bool editorUpdated = false;
    bool waitingForCursor = false;
    int expectedCursor = -1;
    int delayMs = 0;
    QString expectedEditorText;
    QElapsedTimer elapsed;
    QElapsedTimer sinceCommit;

    QObject *context() const {
        QSet<QObject *> found;
        for (QWindow *window : QGuiApplication::allWindows()) {
            auto *view = qobject_cast<QQuickView *>(window);
            if (!view || !view->rootObject()) continue;
            auto *qml = QQmlEngine::contextForObject(view->rootObject());
            if (!qml) continue;
            QObject *im = qml->contextProperty("MInputMethodQuick").value<QObject *>();
            if (im && im->property("active").toBool()) found.insert(im);
        }
        return found.size() == 1 ? *found.begin() : nullptr;
    }
    bool compatible(QObject *im) const {
        return im && im->metaObject()->indexOfMethod("sendCommit(QString,int,int,int)") >= 0
            && im->metaObject()->indexOfSignal("focusTargetChanged(bool)") >= 0
            && im->metaObject()->indexOfSignal("inputMethodReset()") >= 0;
    }
    void reply(QLocalSocket *socket, QJsonObject result) {
        socket->write(QJsonDocument(result).toJson(QJsonDocument::Compact) + '\n');
        socket->disconnectFromServer();
    }
    QJsonObject failure(const QString &code, const QString &message) const {
        return QJsonObject{{"ok",false},{"error",QJsonObject{{"code",code},{"message",message}}},
            {"data",QJsonObject{{"backend","maliit"},{"submittedCodePoints",submitted},
                               {"requestedCodePoints",total},{"replayed",false}}}};
    }
    void finish(QJsonObject result) {
        timer.stop();
        if (target) disconnect(target, nullptr, this, nullptr);
        target.clear();
        expectedEditorText.clear();
        auto socket = pending;
        pending.clear(); chunks.clear();
        if (socket) reply(socket, result);
    }
    void advance() {
        if (!pending || pending->state() != QLocalSocket::ConnectedState) {
            finish(failure("OUTCOME_UNKNOWN","Input cancelled after client disconnect")); return;
        }
        if (focusChanged || !target || !target->property("active").toBool() || context() != target) {
            finish(failure(submitted ? "OUTCOME_UNKNOWN" : "INPUT_NOT_FOCUSED",
                           "Text focus changed; remaining input cancelled")); return;
        }
        if (elapsed.elapsed() > 11000) {
            finish(failure("OUTCOME_UNKNOWN","Text deadline exceeded; remaining input cancelled")); return;
        }
        if (waitingForCursor) {
            if (!editorUpdated || target->property("cursorPosition").toInt() != expectedCursor
                || target->property("surroundingText").toString() != expectedEditorText) {
                if (sinceCommit.elapsed() > 1500) {
                    finish(failure("OUTCOME_UNKNOWN","Editor did not confirm the expected text/cursor; remaining input cancelled"));
                }
                return;
            }
            if (sinceCommit.elapsed() < delayMs) return;
            waitingForCursor = false;
        }
        if (chunks.isEmpty()) {
            finish(QJsonObject{{"ok",true},{"data",QJsonObject{{"backend","maliit"},
                {"submittedCodePoints",submitted},{"requestedCodePoints",total},
                {"delivery","submitted"},{"replayed",false}}}}); return;
        }
        const QString chunk = chunks.takeFirst();
        int cursor = target->property("cursorPosition").toInt();
        int selectionEnd = cursor;
        if (target->property("hasSelection").toBool()) {
            const int anchor = target->property("anchorPosition").toInt();
            selectionEnd = qMax(cursor,anchor);
            cursor = qMin(cursor,anchor);
        }
        expectedEditorText = target->property("surroundingText").toString();
        expectedEditorText.replace(cursor,selectionEnd-cursor,chunk);
        expectedCursor = cursor + chunk.size();
        editorUpdated = false;
        sinceCommit.start();
        if (!QMetaObject::invokeMethod(target,"sendCommit",Qt::DirectConnection,
            Q_ARG(QString,chunk),Q_ARG(int,0),Q_ARG(int,0),Q_ARG(int,-1))) {
            finish(failure(submitted ? "OUTCOME_UNKNOWN" : "CAPABILITY_UNAVAILABLE",
                           "Maliit commit method is unavailable")); return;
        }
        submitted += chunk.toUcs4().size();
        waitingForCursor = delayMs > 0;
        if (chunks.isEmpty() && !waitingForCursor) {
            finish(QJsonObject{{"ok",true},{"data",QJsonObject{{"backend","maliit"},
                {"submittedCodePoints",submitted},{"requestedCodePoints",total},
                {"delivery","submitted"},{"replayed",false}}}});
        }
    }
    void dispatch(QLocalSocket *socket, const QByteArray &frame) {
        QJsonParseError error;
        auto document = QJsonDocument::fromJson(frame,&error);
        if (error.error != QJsonParseError::NoError || !document.isObject()) {
            reply(socket,failure("INVALID_ARGUMENT","Expected one JSON object")); return;
        }
        auto request = document.object();
        const auto command = request.value("command").toString();
        QObject *im = context();
        if (command == "input_status") {
            reply(socket,QJsonObject{{"ok",true},{"data",QJsonObject{{"backend","maliit"},
                {"protocolVersion",1},{"available",true},{"active",compatible(im)}}}}); return;
        }
        if (command != "text") {
            reply(socket,failure("INVALID_ARGUMENT","Only text and input_status are supported")); return;
        }
        if (pending) { reply(socket,failure("INPUT_BUSY","Another text request is in progress")); return; }
        submitted = total = 0;
        for (auto it = request.begin(); it != request.end(); ++it) {
            if (it.key() != "command" && it.key() != "text" && it.key() != "delay_ms") {
                reply(socket,failure("INVALID_ARGUMENT","Unknown text option")); return;
            }
        }
        if (!request.value("text").isString() || !request.value("delay_ms").isDouble()) {
            reply(socket,failure("INVALID_ARGUMENT","text and delay_ms are required")); return;
        }
        const QString text = request.value("text").toString();
        const double delay = request.value("delay_ms").toDouble();
        const auto points = text.toUcs4();
        total = points.size();
        if (text.toUtf8().size() > 16384 || total > 4096 || delay < 0 || delay > 1000
            || delay != int(delay) || (qMax(total-1,0) * delay) > 10000) {
            reply(socket,failure("INVALID_ARGUMENT","Text limit: 4096 code points, 16 KiB, delay 0..1000 ms, total delay <=10 s")); return;
        }
        for (uint point : points) {
            if ((point < 32 && point != 9 && point != 10) || (point >= 127 && point <= 159)) {
                reply(socket,failure("INVALID_ARGUMENT","Control characters other than tab and newline are unsupported")); return;
            }
        }
        if (!compatible(im)) {
            reply(socket,failure("INPUT_NOT_FOCUSED","Focus a text field and open the keyboard")); return;
        }
        if (delay > 0 && (!im->property("surroundingTextValid").toBool()
            || im->property("cursorPosition").toInt() < 0
            || im->metaObject()->indexOfSignal("editorStateUpdate()") < 0)) {
            reply(socket,failure("CAPABILITY_UNAVAILABLE","Paced input requires editor cursor feedback; use --delay 0. Nothing was sent")); return;
        }
        if (text.isEmpty()) {
            reply(socket,QJsonObject{{"ok",true},{"data",QJsonObject{{"backend","maliit"},
                {"submittedCodePoints",0},{"requestedCodePoints",0},{"delivery","submitted"}}}}); return;
        }
        // A commit containing only '\n' is translated to a Return press by
        // Maliit. Merge newline runs with adjacent text for literal commits.
        chunks.clear();
        if (delay == 0) chunks.append(text);
        else {
            QString linebreaks;
            for (uint point : points) {
                const QString one = QString::fromUcs4(&point,1);
                if (point == 10) linebreaks += one;
                else { chunks.append(linebreaks + one); linebreaks.clear(); }
            }
            if (!linebreaks.isEmpty()) {
                if (!chunks.isEmpty()) chunks.last() += linebreaks;
                else {
                    reply(socket,failure("INVALID_ARGUMENT","Use audb key enter for newline-only input")); return;
                }
            }
        }
        if (text == "\n") {
            reply(socket,failure("INVALID_ARGUMENT","Use audb key enter for newline-only input")); chunks.clear(); return;
        }
        pending = socket; target = im; focusChanged = false;
        delayMs = int(delay); waitingForCursor = false;
        elapsed.start();
        connect(im,SIGNAL(focusTargetChanged(bool)),this,SLOT(cancelFocus()));
        connect(im,SIGNAL(inputMethodReset()),this,SLOT(cancelFocus()));
        connect(im,SIGNAL(editorStateUpdate()),this,SLOT(editorStateReceived()));
        timer.start(10);
        advance();
    }
private slots:
    void cancelFocus() { focusChanged = true; }
    void editorStateReceived() { editorUpdated = true; }
public:
    explicit InputBridge(QObject *parent = nullptr) : QObject(parent) {
        connect(&timer,&QTimer::timeout,this,&InputBridge::advance);
        const QString path = QString("/run/user/%1/audb-input.sock").arg(getuid());
        server.setSocketOptions(QLocalServer::UserAccessOption);
        QLocalSocket probe;
        probe.connectToServer(path);
        if (probe.waitForConnected(50)) return;
        QLocalServer::removeServer(path);
        if (!server.listen(path)) return;
        connect(&server,&QLocalServer::newConnection,this,[this]() {
            while (server.hasPendingConnections()) {
                QLocalSocket *socket = server.nextPendingConnection();
                struct ucred cred;
                socklen_t size = sizeof(cred);
                if (connections >= 4 || getsockopt(socket->socketDescriptor(),SOL_SOCKET,SO_PEERCRED,&cred,&size)
                    || (cred.uid != getuid() && cred.uid != 0)) {
                    socket->abort(); socket->deleteLater(); continue;
                }
                ++connections;
                socket->setReadBufferSize(65537);
                connect(socket,&QLocalSocket::disconnected,this,[this,socket]() {
                    --connections;
                    if (pending == socket) finish(failure("OUTCOME_UNKNOWN","Input client disconnected"));
                    socket->deleteLater();
                });
                QTimer::singleShot(12000,socket,[socket]() { socket->disconnectFromServer(); });
                auto buffer = QSharedPointer<QByteArray>::create();
                auto dispatched = QSharedPointer<bool>::create(false);
                connect(socket,&QLocalSocket::readyRead,this,[this,socket,buffer,dispatched]() {
                    if (*dispatched) return;
                    buffer->append(socket->readAll());
                    const int end = buffer->indexOf('\n');
                    if (buffer->size() > 65536) {
                        *dispatched = true;
                        reply(socket,failure("INVALID_ARGUMENT","Input frame exceeds 64 KiB"));
                    } else if (end >= 0) {
                        *dispatched = true;
                        dispatch(socket,buffer->left(end));
                    }
                });
            }
        });
    }
};
class InputPlugin : public QQmlExtensionPlugin {
    Q_OBJECT
    Q_PLUGIN_METADATA(IID "org.qt-project.Qt.QQmlExtensionInterface")
public:
    void registerTypes(const char *uri) override { qmlRegisterType<InputBridge>(uri,1,0,"InputBridge"); }
};
#include "bridge.moc"
