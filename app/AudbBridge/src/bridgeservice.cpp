#include "bridgeservice.h"

#include "bridgeadaptor.h"
#include "streamcamera_compat.h"

#include <auroraapp.h>

#include <QCoreApplication>
#include <QClipboard>
#include <QEventLoop>
#include <QDebug>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusInterface>
#include <QDBusMessage>
#include <QDBusReply>
#include <QDBusUnixFileDescriptor>
#include <QDateTime>
#include <QElapsedTimer>
#include <QDir>
#include <QFileInfo>
#include <QFile>
#include <QImage>
#include <QQmlComponent>
#include <QQmlContext>
#include <QGuiApplication>
#include <QMetaObject>
#include <QMetaMethod>
#include <QProcess>
#include <QQuickView>
#include <QPointer>
#include <QScreen>
#include <QStandardPaths>
#include <QTransform>
#include <QByteArray>
#include <QVariant>
#include <QTimer>

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <fcntl.h>
#include <mutex>
#include <poll.h>
#include <unistd.h>

namespace {

const char kServiceName[] = "ru.kotdath.AudbBridge";
const char kObjectPath[] = "/ru/kotdath/AudbBridge";
const char kScreenshotService[] = "org.nemomobile.lipstick";
const char kScreenshotPath[] = "/org/nemomobile/lipstick/screenshot";
const char kScreenshotInterface[] = "org.nemomobile.lipstick";
const char kScreenGrabService[] = "ru.auroraos.ScreenGrab1.Backend";
const char kScreenGrabPath[] = "/ru/auroraos/ScreenGrab1/Backend";
const char kScreenGrabInterface[] = "ru.auroraos.ScreenGrab1.Backend";

constexpr int kOrientationPortrait = 1;

#ifdef AUDB_HAS_STREAMCAMERA
int compareCapabilities(const Aurora::StreamCamera::CameraCapability &left,
                        const Aurora::StreamCamera::CameraCapability &right)
{
    const quint64 leftArea = quint64(left.width) * quint64(left.height);
    const quint64 rightArea = quint64(right.width) * quint64(right.height);
    if (leftArea != rightArea) {
        return leftArea < rightArea ? -1 : 1;
    }
    if (left.fps != right.fps) {
        return left.fps < right.fps ? -1 : 1;
    }
    if (left.width != right.width) {
        return left.width < right.width ? -1 : 1;
    }
    if (left.height != right.height) {
        return left.height < right.height ? -1 : 1;
    }
    return 0;
}

inline int clampColor(int value)
{
    return std::clamp(value, 0, 255);
}

QImage rotateImageIfNeeded(const QImage &image, uint16_t rotation)
{
    if (image.isNull()) {
        return image;
    }

    if (rotation == 90 || rotation == 180 || rotation == 270) {
        QTransform transform;
        transform.rotate(rotation);
        return image.transformed(transform);
    }

    return image;
}

QImage imageFromYCbCrFrame(const Aurora::StreamCamera::YCbCrFrame &frame, QString *errorMessage)
{
    if (!frame.y || !frame.cb) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera frame does not expose Y/Cb planes.");
        }
        return {};
    }

    if (frame.width == 0 || frame.height == 0) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera frame has invalid dimensions.");
        }
        return {};
    }

    if (frame.chromaStep != 1 && frame.chromaStep != 2) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("Unsupported YCbCr chromaStep=%1.").arg(frame.chromaStep);
        }
        return {};
    }

    if (frame.yStride == 0 || frame.cStride == 0) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera frame has invalid Y/C strides.");
        }
        return {};
    }

    const uint8_t *cbBase = frame.cb;
    const uint8_t *crBase = frame.cr;
    if (frame.chromaStep == 1) {
        if (!crBase) {
            if (errorMessage) {
                *errorMessage = QStringLiteral("I420 frame does not expose the Cr plane.");
            }
            return {};
        }
    } else if (!crBase) {
        // Screen capture commonly uses NV12, where the chroma samples are interleaved.
        crBase = cbBase + 1;
    }

    QImage image(frame.width, frame.height, QImage::Format_RGB32);
    if (image.isNull()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("Failed to allocate QImage for StreamCamera frame.");
        }
        return {};
    }

    for (int y = 0; y < frame.height; ++y) {
        QRgb *scanLine = reinterpret_cast<QRgb *>(image.scanLine(y));
        const uint8_t *yRow = frame.y + (y * frame.yStride);
        const uint8_t *cbRow = cbBase + ((y / 2) * frame.cStride);
        const uint8_t *crRow = crBase + ((y / 2) * frame.cStride);

        for (int x = 0; x < frame.width; ++x) {
            const int chromaOffset = (x / 2) * frame.chromaStep;
            const int luma = int(yRow[x]);
            const int cb = int(cbRow[chromaOffset]) - 128;
            const int cr = int(crRow[chromaOffset]) - 128;

            const int red = clampColor(luma + ((359 * cr) >> 8));
            const int green = clampColor(luma - ((88 * cb + 183 * cr) >> 8));
            const int blue = clampColor(luma + ((454 * cb) >> 8));
            scanLine[x] = qRgb(red, green, blue);
        }
    }

    return image;
}

QImage imageFromGraphicBuffer(const std::shared_ptr<Aurora::StreamCamera::GraphicBuffer> &buffer,
                              QString *errorMessage)
{
    if (!buffer) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera delivered a null GraphicBuffer.");
        }
        return {};
    }

    std::shared_ptr<const Aurora::StreamCamera::YCbCrFrame> frame = buffer->mapYCbCr();
    if (!frame) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("GraphicBuffer::mapYCbCr() returned null.");
        }
        return {};
    }

    return rotateImageIfNeeded(imageFromYCbCrFrame(*frame, errorMessage), buffer->rotation());
}

class SingleFrameCameraListener final : public Aurora::StreamCamera::CameraListener
{
public:
    void onCameraFrame(std::shared_ptr<Aurora::StreamCamera::GraphicBuffer> buffer) override
    {
        QString localError;
        const QImage localImage = imageFromGraphicBuffer(buffer, &localError);

        {
            std::lock_guard<std::mutex> guard(m_mutex);
            if (m_completed) {
                return;
            }
            m_completed = true;
            m_image = localImage;
            m_errorMessage = localImage.isNull()
                    ? (localError.isEmpty() ? QStringLiteral("Failed to decode StreamCamera frame.") : localError)
                    : QString();
        }

        m_condition.notify_all();
    }

    void onCameraError(const std::string &errorDescription) override
    {
        {
            std::lock_guard<std::mutex> guard(m_mutex);
            if (m_completed) {
                return;
            }
            m_completed = true;
            m_errorMessage = QStringLiteral("StreamCamera error: %1")
                    .arg(QString::fromStdString(errorDescription));
        }

        m_condition.notify_all();
    }

    void onCameraParameterChanged(Aurora::StreamCamera::CameraParameter, const std::string &) override
    {
    }

    bool waitForFrame(int timeoutMs, QImage *image, QString *errorMessage)
    {
        std::unique_lock<std::mutex> lock(m_mutex);
        if (!m_condition.wait_for(lock,
                                  std::chrono::milliseconds(timeoutMs),
                                  [this]() { return m_completed; })) {
            if (errorMessage) {
                *errorMessage = QStringLiteral("Timed out waiting for StreamCamera frame.");
            }
            return false;
        }

        if (m_image.isNull()) {
            if (errorMessage) {
                *errorMessage = m_errorMessage.isEmpty()
                        ? QStringLiteral("StreamCamera did not provide a frame.")
                        : m_errorMessage;
            }
            return false;
        }

        if (image) {
            *image = m_image;
        }
        return true;
    }

private:
    std::mutex m_mutex;
    std::condition_variable m_condition;
    bool m_completed = false;
    QImage m_image;
    QString m_errorMessage;
};
#endif

QString describePrefix(const QByteArray &payload)
{
    const QByteArray bytes = payload.left(16).toHex();
    if (bytes.isEmpty()) {
        return QStringLiteral("<empty>");
    }

    QStringList parts;
    for (int index = 0; index + 1 < bytes.size(); index += 2) {
        parts.append(QString::fromLatin1(bytes.mid(index, 2)));
    }
    return parts.join(QStringLiteral(" "));
}

QByteArray readPipePayload(int fd, int timeoutMs, QString *errorMessage)
{
    if (fd < 0) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("Invalid videopipe file descriptor.");
        }
        return {};
    }

    const int oldFlags = fcntl(fd, F_GETFL, 0);
    if (oldFlags >= 0) {
        fcntl(fd, F_SETFL, oldFlags | O_NONBLOCK);
    }

    QByteArray payload;
    payload.reserve(256 * 1024);

    constexpr int kMaxBytes = 16 * 1024 * 1024; // 16MB for 720x1600 YUV
    constexpr int kIdleTimeoutMs = 1000; // Increased from 200ms

    int remaining = timeoutMs;
    bool sawData = false;

    while (remaining > 0 && payload.size() < kMaxBytes) {
        pollfd descriptor;
        descriptor.fd = fd;
        descriptor.events = POLLIN | POLLHUP | POLLERR;
        descriptor.revents = 0;

        const int waitMs = sawData ? kIdleTimeoutMs : remaining;
        const int pollResult = poll(&descriptor, 1, waitMs);
        if (pollResult < 0) {
            if (errno == EINTR) {
                continue;
            }
            if (errorMessage) {
                *errorMessage = QStringLiteral("poll() failed: %1").arg(QString::fromLocal8Bit(strerror(errno)));
            }
            break;
        }

        remaining -= waitMs;
        if (pollResult == 0) {
            if (sawData) {
                break;
            }
            continue;
        }

        if (descriptor.revents & POLLERR) {
            if (errorMessage) {
                *errorMessage = QStringLiteral("videopipe reported POLLERR.");
            }
            break;
        }

        if (descriptor.revents & (POLLIN | POLLHUP)) {
            char buffer[16384];
            while (payload.size() < kMaxBytes) {
                const ssize_t bytesRead = ::read(fd, buffer, sizeof(buffer));
                if (bytesRead > 0) {
                    payload.append(buffer, int(bytesRead));
                    sawData = true;
                    continue;
                }
                if (bytesRead == 0) {
                    remaining = 0;
                    break;
                }
                if (errno == EINTR) {
                    continue;
                }
                if (errno == EAGAIN || errno == EWOULDBLOCK) {
                    break;
                }
                if (errorMessage) {
                    *errorMessage = QStringLiteral("read() failed: %1")
                            .arg(QString::fromLocal8Bit(strerror(errno)));
                }
                remaining = 0;
                break;
            }
        }
    }

    if (oldFlags >= 0) {
        fcntl(fd, F_SETFL, oldFlags);
    }

    if (payload.isEmpty() && errorMessage && errorMessage->isEmpty()) {
        *errorMessage = QStringLiteral("videopipe produced no data.");
    }

    return payload;
}

bool savePayloadAsImage(const QByteArray &payload, const QString &finalPath, QString *errorMessage)
{
    if (payload.isEmpty()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("videopipe returned an empty payload.");
        }
        return false;
    }

    QImage image;
    if (image.loadFromData(payload)) {
        if (image.save(finalPath)) {
            return true;
        }
        if (errorMessage) {
            *errorMessage = QStringLiteral("Decoded image but failed to save to %1.").arg(finalPath);
        }
        return false;
    }

    if (payload.startsWith("\x89PNG\r\n\x1a\n")) {
        QFile file(finalPath);
        if (file.open(QIODevice::WriteOnly | QIODevice::Truncate) && file.write(payload) == payload.size()) {
            return true;
        }
        if (errorMessage) {
            *errorMessage = QStringLiteral("Received PNG payload but failed to write %1.").arg(finalPath);
        }
        return false;
    }

    // Try to decode Aurora videopipe format (20-byte header + YUV data)
    // Header format (based on observation):
    // bytes 0-1: version or type
    // bytes 2-3: unknown
    // bytes 4-5: padding?
    // bytes 6-7: width (uint16 little-endian) 0x02d0 = 720
    // bytes 8-9: padding?
    // bytes 10-11: height (uint16 little-endian) 0x0640 = 1600
    // ...followed by YUV420 data
    if (payload.size() > 20) {
        const uint8_t *data = reinterpret_cast<const uint8_t *>(payload.constData());
        const uint16_t width = *reinterpret_cast<const uint16_t *>(data + 6);
        const uint16_t height = *reinterpret_cast<const uint16_t *>(data + 10);
        const uint16_t format = *reinterpret_cast<const uint16_t *>(data + 16);

        qInfo() << "[AudbBridge] Videopipe header: width=" << width << "height=" << height << "format=" << format;

        // NV12 or I420: Y plane followed by UV planes
        // For 720x1600 NV12: Y=720*1600, UV=720*1600/2, total ~2.7MB
        const int ySize = width * height;
        const int uvSize = ySize / 2;
        const int expectedSize = 20 + ySize + uvSize;

        if (payload.size() >= expectedSize) {
            QImage rgbImage(width, height, QImage::Format_RGB32);
            if (!rgbImage.isNull()) {
                const uint8_t *yPlane = data + 20;
                const uint8_t *uvPlane = yPlane + ySize;

                for (int y = 0; y < height; ++y) {
                    QRgb *scanLine = reinterpret_cast<QRgb *>(rgbImage.scanLine(y));
                    for (int x = 0; x < width; ++x) {
                        const int yVal = yPlane[y * width + x];
                        const int uvOffset = (y / 2) * width + (x / 2) * 2;
                        const int u = uvPlane[uvOffset] - 128;
                        const int v = uvPlane[uvOffset + 1] - 128;

                        // YUV to RGB conversion
                        const int r = qBound(0, yVal + ((359 * v) >> 8), 255);
                        const int g = qBound(0, yVal - ((88 * u + 183 * v) >> 8), 255);
                        const int b = qBound(0, yVal + ((454 * u) >> 8), 255);

                        scanLine[x] = qRgb(r, g, b);
                    }
                }

                if (rgbImage.save(finalPath)) {
                    qInfo() << "[AudbBridge] Saved videopipe YUV as" << finalPath;
                    return true;
                }
                if (errorMessage) {
                    *errorMessage = QStringLiteral("Decoded videopipe YUV but failed to save %1.").arg(finalPath);
                }
                return false;
            }
        }
    }

    if (errorMessage) {
        // Dump full payload for analysis
        QString payloadHex = QString::fromLatin1(payload.toHex());
        qInfo() << "[AudbBridge] Videopipe full payload:" << payloadHex.left(200);

        *errorMessage = QStringLiteral("Unsupported videopipe payload, size=%1, prefix=%2")
                .arg(payload.size())
                .arg(describePrefix(payload));
    }
    return false;
}

}

BridgeService::BridgeService(QObject *parent)
    : QObject(parent)
    , m_executor(QCoreApplication::applicationFilePath())
    , m_statusMessage(QStringLiteral("Bridge is ready."))
{
    if (auto *view = qobject_cast<QQuickView *>(parent)) {
        m_qmlEngine = view->engine();
    }

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

void BridgeService::setScreenshotGrabber(QObject *grabber)
{
    if (m_screenshotGrabber == grabber) {
        return;
    }

    m_screenshotGrabber = grabber;
    qInfo() << "[AudbBridge] screenshot grabber updated" << grabber;
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

bool BridgeService::setClipboardText(const QString &text)
{
    QClipboard *clipboard = QGuiApplication::clipboard();
    if (!clipboard) {
        setStatusMessage(QStringLiteral("Clipboard is not available."));
        emit stateChanged();
        return false;
    }

    clipboard->setText(text, QClipboard::Clipboard);
    setStatusMessage(QStringLiteral("Clipboard text updated."));
    emit stateChanged();
    return clipboard->text(QClipboard::Clipboard) == text;
}

QString BridgeService::clipboardText() const
{
    QClipboard *clipboard = QGuiApplication::clipboard();
    if (!clipboard) {
        return {};
    }
    return clipboard->text(QClipboard::Clipboard);
}

QString BridgeService::screenshot(const QString &outputPath)
{
    const QString finalPath = outputPath.trimmed().isEmpty() ? defaultScreenshotPath() : outputPath;

    QDir().mkpath(QFileInfo(finalPath).absolutePath());

    QString streamCameraError;
    if (tryScreenshotViaStreamCamera(finalPath, &streamCameraError)) {
        setStatusMessage(QStringLiteral("Screenshot saved to %1 via StreamCamera.").arg(finalPath));
        emit stateChanged();
        return finalPath;
    }

    if (!streamCameraError.isEmpty()) {
        qWarning() << "[AudbBridge] StreamCamera screenshot unavailable, falling back to QML:"
                   << streamCameraError;
    }

    QString qmlError;
    if (tryScreenshotViaQml(finalPath, &qmlError)) {
        setStatusMessage(QStringLiteral("Screenshot saved to %1 via QML.").arg(finalPath));
        emit stateChanged();
        return finalPath;
    }

    if (!qmlError.isEmpty()) {
        qWarning() << "[AudbBridge] QML screenshot unavailable, falling back to DBus:"
                   << qmlError;
    }

    const QString fallbackResult = screenshotViaDbus(finalPath);
    if (fallbackResult.isEmpty()) {
        const QString fallbackError = m_statusMessage;
        QStringList failures;
        if (!streamCameraError.isEmpty()) {
            failures.append(QStringLiteral("StreamCamera failed: %1").arg(streamCameraError));
        }
        if (!qmlError.isEmpty()) {
            failures.append(QStringLiteral("QML failed: %1").arg(qmlError));
        }
        failures.append(QStringLiteral("fallback failed: %1").arg(fallbackError));
        setStatusMessage(failures.join(QStringLiteral("; ")));
        emit stateChanged();
    }
    return fallbackResult;
}

bool BridgeService::tryScreenshotViaStreamCamera(const QString &finalPath, QString *errorMessage)
{
    qInfo() << "[AudbBridge] tryScreenshotViaStreamCamera called";
#ifndef AUDB_HAS_STREAMCAMERA
    qWarning() << "[AudbBridge] StreamCamera support was not enabled at build time";
    if (errorMessage) {
        *errorMessage = QStringLiteral("StreamCamera support was not enabled at build time.");
    }
    Q_UNUSED(finalPath);
    return false;
#else
    qInfo() << "[AudbBridge] Getting StreamCameraManager...";
    Aurora::StreamCamera::CameraManager *manager = StreamCameraManager();
    if (!manager) {
        qWarning() << "[AudbBridge] StreamCameraManager() returned null";
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCameraManager() returned null.");
        }
        return false;
    }
    qInfo() << "[AudbBridge] Got StreamCameraManager:" << manager;

    const int cameraCount = manager->getNumberOfCameras();
    qInfo() << "[AudbBridge] StreamCamera found" << cameraCount << "cameras";

    if (cameraCount <= 0) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera returned no cameras.");
        }
        return false;
    }

    Aurora::StreamCamera::CameraInfo screenInfo;
    bool screenFound = false;
    for (int index = 0; index < cameraCount; ++index) {
        Aurora::StreamCamera::CameraInfo info;
        if (!manager->getCameraInfo(unsigned(index), info)) {
            qWarning() << "[AudbBridge] StreamCamera getCameraInfo failed for index" << index;
            continue;
        }

        qInfo() << "[AudbBridge] Camera" << index << ": id=" << QString::fromStdString(info.id)
                << "facing=" << int(info.facing);

        if (info.facing == Aurora::StreamCamera::CameraFacing::Screen) {
            screenInfo = info;
            screenFound = true;
            break;
        }
    }

    if (!screenFound) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("No CameraFacing::Screen camera was exposed by StreamCamera.");
        }
        return false;
    }

    std::vector<Aurora::StreamCamera::CameraCapability> capabilities;
    if (!manager->queryCapabilities(screenInfo.id, capabilities) || capabilities.empty()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera returned no capture capabilities for screen camera %1.")
                    .arg(QString::fromStdString(screenInfo.id));
        }
        return false;
    }

    const auto bestCapability = *std::max_element(capabilities.cbegin(),
                                                  capabilities.cend(),
                                                  [](const auto &left, const auto &right) {
        return compareCapabilities(left, right) < 0;
    });

    std::shared_ptr<Aurora::StreamCamera::Camera> camera = manager->openCamera(screenInfo.id);
    if (!camera) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("Failed to open screen camera %1.")
                    .arg(QString::fromStdString(screenInfo.id));
        }
        return false;
    }

    SingleFrameCameraListener listener;
    camera->setListener(&listener);

    bool started = camera->startCapture(bestCapability);
    Aurora::StreamCamera::PixelFormat selectedFormat = Aurora::StreamCamera::PixelFormat::Invalid;
    if (!started) {
        const Aurora::StreamCamera::PixelFormat candidateFormats[] = {
            Aurora::StreamCamera::PixelFormat::YCbCrFlexible,
            Aurora::StreamCamera::PixelFormat::YUV420SemiPlanar,
            Aurora::StreamCamera::PixelFormat::YUV420Planar,
        };

        for (const Aurora::StreamCamera::PixelFormat format : candidateFormats) {
            if (camera->startCapture(bestCapability, format)) {
                started = true;
                selectedFormat = format;
                break;
            }
        }
    }

    if (!started) {
        camera->setListener(nullptr);
        if (errorMessage) {
            *errorMessage = QStringLiteral("Failed to start StreamCamera capture for %1x%2@%3.")
                    .arg(bestCapability.width)
                    .arg(bestCapability.height)
                    .arg(bestCapability.fps);
        }
        return false;
    }

    QImage image;
    QString captureError;
    const bool frameReceived = listener.waitForFrame(5000, &image, &captureError);

    camera->stopCapture();
    camera->setListener(nullptr);

    if (!frameReceived) {
        if (errorMessage) {
            *errorMessage = captureError.isEmpty()
                    ? (selectedFormat == Aurora::StreamCamera::PixelFormat::Invalid
                               ? QStringLiteral("StreamCamera capture started but produced no frame.")
                               : QStringLiteral("StreamCamera capture started with format %1 but produced no frame.")
                                         .arg(int(selectedFormat)))
                    : captureError;
        }
        return false;
    }

    if (!image.save(finalPath)) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("StreamCamera decoded a frame but failed to save %1.")
                    .arg(finalPath);
        }
        return false;
    }

    return QFileInfo::exists(finalPath);
#endif
}

bool BridgeService::tryScreenshotViaQml(const QString &finalPath, QString *errorMessage)
{
    if (!m_screenshotGrabber) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("QML screenshot grabber is not available.");
        }
        return false;
    }

    QEventLoop loop;
    QTimer timeout;
    QTimer filePoll;
    timeout.setSingleShot(true);
    filePoll.setInterval(100);
    QObject::connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
    QObject::connect(&filePoll, &QTimer::timeout, &loop, [&loop, finalPath]() {
        if (QFileInfo::exists(finalPath)) {
            loop.quit();
        }
    });
    QObject::connect(m_screenshotGrabber, SIGNAL(finished()), &loop, SLOT(quit()));

    const bool invoked = QMetaObject::invokeMethod(m_screenshotGrabber,
                                                   "capture",
                                                   Qt::QueuedConnection,
                                                   Q_ARG(QVariant, QVariant(finalPath)));
    if (!invoked) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("Failed to invoke ScreenshotGrabber.capture().");
        }
        return false;
    }

    timeout.start(5000);
    filePoll.start();
    loop.exec();
    filePoll.stop();

    const bool fileExists = QFileInfo::exists(finalPath);
    if (timeout.isActive()) {
        timeout.stop();
        if (fileExists) {
            return true;
        }
    } else if (fileExists) {
        return true;
    }

    const bool succeeded = m_screenshotGrabber->property("succeeded").toBool();
    if (!succeeded) {
        if (errorMessage) {
            const QString lastError = m_screenshotGrabber->property("lastError").toString();
            const bool completed = m_screenshotGrabber->property("completed").toBool();
            *errorMessage = lastError.isEmpty()
                    ? (completed
                               ? QStringLiteral("ScreenshotGrabber reported failure.")
                               : QStringLiteral("Timed out waiting for QML screenshot completion."))
                    : lastError;
        }
        return false;
    }

    if (!fileExists) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("ScreenshotGrabber reported success but file was not created.");
        }
        return false;
    }

    return true;
}

bool BridgeService::tryScreenshotViaScreenGrab(const QString &finalPath, QString *errorMessage)
{
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!(bus.connectionCapabilities() & QDBusConnection::UnixFileDescriptorPassing)) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("Session bus does not support Unix FD passing.");
        }
        return false;
    }

    if (!QDBusUnixFileDescriptor::isSupported()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("QDBusUnixFileDescriptor is not supported in this runtime.");
        }
        return false;
    }

    QDBusInterface iface(QString::fromLatin1(kScreenGrabService),
                         QString::fromLatin1(kScreenGrabPath),
                         QString::fromLatin1(kScreenGrabInterface),
                         bus);
    if (!iface.isValid()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("ScreenGrab backend is not available on the session bus.");
        }
        return false;
    }

    // Call GetScreenInfo first to initialize
    qInfo() << "[AudbBridge] Calling GetScreenInfo...";
    QDBusMessage screenInfoReply = iface.call(QStringLiteral("GetScreenInfo"));
    if (screenInfoReply.type() == QDBusMessage::ErrorMessage) {
        qWarning() << "[AudbBridge] GetScreenInfo failed:" << screenInfoReply.errorMessage();
    } else {
        qInfo() << "[AudbBridge] GetScreenInfo success:" << screenInfoReply.arguments();
    }

    const QString requestId = QStringLiteral("audb-%1").arg(QCoreApplication::applicationPid());
    const QVariantMap params;
    qInfo() << "[AudbBridge] Requesting videopipe with id:" << requestId;
    const QDBusMessage reply = iface.call(QStringLiteral("RequestVideoPipe"), requestId, params);
    if (reply.type() == QDBusMessage::ErrorMessage) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("RequestVideoPipe failed: %1").arg(reply.errorMessage());
        }
        return false;
    }

    if (reply.arguments().isEmpty()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("RequestVideoPipe returned no arguments.");
        }
        return false;
    }

    const QVariant descriptorVariant = reply.arguments().constFirst();
    if (!descriptorVariant.canConvert<QDBusUnixFileDescriptor>()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("RequestVideoPipe returned unsupported type %1.")
                    .arg(QString::fromLatin1(descriptorVariant.typeName()));
        }
        iface.call(QStringLiteral("Stop"));
        return false;
    }

    QDBusUnixFileDescriptor descriptor = descriptorVariant.value<QDBusUnixFileDescriptor>();
    if (!descriptor.isValid()) {
        if (errorMessage) {
            *errorMessage = QStringLiteral("RequestVideoPipe returned an invalid file descriptor.");
        }
        iface.call(QStringLiteral("Stop"));
        return false;
    }

    qInfo() << "[AudbBridge] Got videopipe fd, reading payload...";
    QString pipeError;
    const QByteArray payload = readPipePayload(descriptor.takeFileDescriptor(), 10000, &pipeError);
    qInfo() << "[AudbBridge] Videopipe payload size:" << payload.size() << "error:" << pipeError;
    iface.call(QStringLiteral("Stop"));

    if (!savePayloadAsImage(payload, finalPath, &pipeError)) {
        if (errorMessage) {
            *errorMessage = pipeError;
        }
        return false;
    }

    return QFileInfo::exists(finalPath);
}

QString BridgeService::screenshotViaDbus(const QString &finalPath)
{
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (QDBusConnectionInterface *ifaceBus = bus.interface()) {
        QDBusReply<bool> registered = ifaceBus->isServiceRegistered(QString::fromLatin1(kScreenshotService));
        if (registered.isValid() && !registered.value()) {
            setStatusMessage(QStringLiteral("Screenshot service is not available on the session bus."));
            emit stateChanged();
            return QString();
        }
    }

    QDBusInterface iface(QString::fromLatin1(kScreenshotService),
                         QString::fromLatin1(kScreenshotPath),
                         QString::fromLatin1(kScreenshotInterface),
                         bus);
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

    setStatusMessage(QStringLiteral("Screenshot saved to %1 via DBus.").arg(finalPath));
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
