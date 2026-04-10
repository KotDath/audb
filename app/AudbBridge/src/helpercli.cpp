#include "helpercli.h"

#include <QByteArray>
#include <QCommandLineParser>
#include <QDebug>
#include <QDir>
#include <QFile>
#include <QPoint>
#include <QProcess>
#include <QRect>
#include <QSize>
#include <QStringList>
#include <QThread>
#include <QTextStream>

#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>

#include <fcntl.h>
#include <linux/input.h>
#include <linux/uinput.h>
#include <sys/time.h>
#include <unistd.h>

namespace {

constexpr int kOrientationPortrait = 1;
constexpr int kOrientationLandscape = 2;
constexpr int kOrientationInvertedPortrait = 4;
constexpr int kOrientationInvertedLandscape = 8;

constexpr int kDefaultXMax = 720;
constexpr int kDefaultYMax = 1440;
constexpr int kDefaultSlotMax = 4;
constexpr int kDefaultTouchMajor = 19;
constexpr int kDefaultWidthMajor = 19;
constexpr int kDefaultSettleMs = 150;
constexpr int kDefaultDownMs = 60;
constexpr int kDefaultReleaseSettleMs = 20;
constexpr int kDefaultSwipeSteps = 28;
constexpr int kDefaultSwipeStepDelayMs = 12;

constexpr double kEdgeStartX = 0.015;
constexpr double kEdgeStartY = 0.985;
constexpr double kEdgeEndX = 0.35;
constexpr double kEdgeEndY = 0.30;
constexpr double kSwipeCenterX = 0.50;
constexpr double kSwipeCenterY = 0.50;
constexpr double kScrollStartY = 0.78;
constexpr double kScrollEndY = 0.22;
constexpr double kLongEdgeEndY = 0.06;

struct TouchConfig
{
    int xMax = kDefaultXMax;
    int yMax = kDefaultYMax;
    int slotMax = kDefaultSlotMax;
    int touchMajor = kDefaultTouchMajor;
    int widthMajor = kDefaultWidthMajor;
    int settleMs = kDefaultSettleMs;
};

int clampValue(int value, int minimum, int maximum)
{
    return std::max(minimum, std::min(maximum, value));
}

void emitEvent(int fd, __u16 type, __u16 code, __s32 value)
{
    input_event event {};
    event.type = type;
    event.code = code;
    event.value = value;
    ::write(fd, &event, sizeof(event));
}

void syncEvents(int fd)
{
    emitEvent(fd, EV_SYN, SYN_REPORT, 0);
}

int openUinput()
{
    const char *candidates[] = {"/dev/uinput", "/dev/input/uinput"};
    for (const char *candidate : candidates) {
        const int fd = ::open(candidate, O_WRONLY | O_NONBLOCK);
        if (fd >= 0) {
            return fd;
        }
    }
    return -1;
}

bool configureUinput(int fd, const TouchConfig &config, QTextStream &err)
{
    const auto ioctlOrFail = [&](unsigned long request, int argument, const QString &label) {
        if (::ioctl(fd, request, argument) < 0) {
            err << label << " failed: " << strerror(errno) << '\n';
            return false;
        }
        return true;
    };

    if (!ioctlOrFail(UI_SET_EVBIT, EV_KEY, QStringLiteral("UI_SET_EVBIT(EV_KEY)"))
            || !ioctlOrFail(UI_SET_EVBIT, EV_ABS, QStringLiteral("UI_SET_EVBIT(EV_ABS)"))
            || !ioctlOrFail(UI_SET_EVBIT, EV_SYN, QStringLiteral("UI_SET_EVBIT(EV_SYN)"))
            || !ioctlOrFail(UI_SET_KEYBIT, BTN_TOUCH, QStringLiteral("UI_SET_KEYBIT(BTN_TOUCH)"))
            || !ioctlOrFail(UI_SET_PROPBIT, INPUT_PROP_DIRECT, QStringLiteral("UI_SET_PROPBIT(INPUT_PROP_DIRECT)"))) {
        return false;
    }

    const int absCodes[] = {
        ABS_X, ABS_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID,
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y,
        ABS_MT_TOUCH_MAJOR, ABS_MT_WIDTH_MAJOR
    };
    for (const int code : absCodes) {
        if (!ioctlOrFail(UI_SET_ABSBIT, code, QStringLiteral("UI_SET_ABSBIT"))) {
            return false;
        }
    }

    uinput_user_dev device {};
    std::strncpy(device.name, "audbbridge-uinput-touch", UINPUT_MAX_NAME_SIZE - 1);
    device.id.bustype = BUS_VIRTUAL;
    device.absmin[ABS_X] = 0;
    device.absmax[ABS_X] = config.xMax;
    device.absmin[ABS_Y] = 0;
    device.absmax[ABS_Y] = config.yMax;
    device.absmin[ABS_MT_SLOT] = 0;
    device.absmax[ABS_MT_SLOT] = config.slotMax;
    device.absmin[ABS_MT_TRACKING_ID] = 0;
    device.absmax[ABS_MT_TRACKING_ID] = 65535;
    device.absmin[ABS_MT_POSITION_X] = 0;
    device.absmax[ABS_MT_POSITION_X] = config.xMax;
    device.absmin[ABS_MT_POSITION_Y] = 0;
    device.absmax[ABS_MT_POSITION_Y] = config.yMax;
    device.absmin[ABS_MT_TOUCH_MAJOR] = 0;
    device.absmax[ABS_MT_TOUCH_MAJOR] = 255;
    device.absmin[ABS_MT_WIDTH_MAJOR] = 0;
    device.absmax[ABS_MT_WIDTH_MAJOR] = 255;

    if (::write(fd, &device, sizeof(device)) != sizeof(device)) {
        err << "Failed to write uinput_user_dev: " << strerror(errno) << '\n';
        return false;
    }

    if (::ioctl(fd, UI_DEV_CREATE) < 0) {
        err << "UI_DEV_CREATE failed: " << strerror(errno) << '\n';
        return false;
    }

    QThread::msleep(static_cast<unsigned long>(config.settleMs));
    return true;
}

void destroyUinput(int fd)
{
    if (fd >= 0) {
        ::ioctl(fd, UI_DEV_DESTROY);
        ::close(fd);
    }
}

int nextTrackingId()
{
    const auto now = std::chrono::system_clock::now().time_since_epoch();
    const auto millis = std::chrono::duration_cast<std::chrono::milliseconds>(now).count();
    return static_cast<int>((millis % 60000) + 1);
}

QPoint transformCoordinates(int x, int y, int orientation, int xMax, int yMax)
{
    switch (orientation) {
    case kOrientationLandscape:
        return QPoint(xMax - y, x);
    case kOrientationInvertedPortrait:
        return QPoint(xMax - x, yMax - y);
    case kOrientationInvertedLandscape:
        return QPoint(y, yMax - x);
    case kOrientationPortrait:
    default:
        return QPoint(x, y);
    }
}

QString autoDetectEventDevice()
{
    QDir eventDir(QStringLiteral("/sys/class/input"));
    const QStringList entries = eventDir.entryList({QStringLiteral("event*")},
                                                   QDir::AllEntries | QDir::NoDotAndDotDot,
                                                   QDir::Name);
    for (const QString &entry : entries) {
        QFile nameFile(eventDir.absoluteFilePath(entry + QStringLiteral("/device/name")));
        if (!nameFile.open(QIODevice::ReadOnly | QIODevice::Text)) {
            continue;
        }
        const QString name = QString::fromUtf8(nameFile.readAll()).trimmed().toLower();
        const QStringList fingerprints = {
            QStringLiteral("touch"), QStringLiteral("tpd"), QStringLiteral("ts"),
            QStringLiteral("silead"), QStringLiteral("goodix"), QStringLiteral("fts"),
            QStringLiteral("atmel"), QStringLiteral("synaptics"), QStringLiteral("elan"),
            QStringLiteral("chsc"), QStringLiteral("himax")
        };
        for (const QString &fingerprint : fingerprints) {
            if (name.contains(fingerprint)) {
                qInfo() << "[AudbBridge/helper] autoDetectEventDevice matched" << name << "->" << entry;
                return QStringLiteral("/dev/input/%1").arg(entry);
            }
        }
    }
    qWarning() << "[AudbBridge/helper] autoDetectEventDevice fallback to /dev/input/event3";
    return QStringLiteral("/dev/input/event3");
}

bool performTap(int fd,
                int x,
                int y,
                int downMs,
                const TouchConfig &config)
{
    const int trackingId = nextTrackingId();

    emitEvent(fd, EV_ABS, ABS_MT_SLOT, 0);
    emitEvent(fd, EV_ABS, ABS_MT_TRACKING_ID, trackingId);
    emitEvent(fd, EV_ABS, ABS_MT_POSITION_X, clampValue(x, 0, config.xMax));
    emitEvent(fd, EV_ABS, ABS_MT_POSITION_Y, clampValue(y, 0, config.yMax));
    emitEvent(fd, EV_ABS, ABS_MT_TOUCH_MAJOR, config.touchMajor);
    emitEvent(fd, EV_ABS, ABS_MT_WIDTH_MAJOR, config.widthMajor);
    emitEvent(fd, EV_KEY, BTN_TOUCH, 1);
    syncEvents(fd);

    QThread::msleep(static_cast<unsigned long>(std::max(1, downMs)));

    emitEvent(fd, EV_KEY, BTN_TOUCH, 0);
    emitEvent(fd, EV_ABS, ABS_MT_TRACKING_ID, -1);
    syncEvents(fd);
    return true;
}

bool runTap(const QString &eventDevice,
            int x,
            int y,
            int downMs,
            const TouchConfig &config,
            QTextStream &out,
            QTextStream &err)
{
    const auto tryEventDevice = [&](const QString &requestedDevice) {
        const QString device = (requestedDevice == QStringLiteral("auto")) ? autoDetectEventDevice() : requestedDevice;
        qInfo() << "[AudbBridge/helper] tap backend=evdev"
                << "device=" << device
                << "x=" << x
                << "y=" << y
                << "downMs=" << downMs;
        const int fd = ::open(device.toUtf8().constData(), O_WRONLY);
        if (fd < 0) {
            err << "Failed to open " << device << ": " << strerror(errno) << '\n';
            return false;
        }
        performTap(fd, x, y, downMs, config);
        ::close(fd);
        out << "tap(" << x << "," << y << ") via " << device << '\n';
        return true;
    };

    if (!eventDevice.isEmpty()) {
        return tryEventDevice(eventDevice);
    }

    const int fd = openUinput();
    if (fd >= 0) {
        qInfo() << "[AudbBridge/helper] tap backend=uinput"
                << "x=" << x
                << "y=" << y
                << "downMs=" << downMs;
        if (!configureUinput(fd, config, err)) {
            destroyUinput(fd);
            return false;
        }

        performTap(fd, x, y, downMs, config);
        QThread::msleep(kDefaultReleaseSettleMs);
        destroyUinput(fd);
        out << "tap(" << x << "," << y << ") via uinput" << '\n';
        return true;
    }

    err << "Falling back to direct event device because /dev/uinput is unavailable: "
        << strerror(errno) << '\n';
    return tryEventDevice(QStringLiteral("auto"));
}

bool performSwipe(int fd,
                  int x0,
                  int y0,
                  int x1,
                  int y1,
                  int steps,
                  int stepDelayMs,
                  const TouchConfig &config)
{
    const int trackingId = nextTrackingId();
    emitEvent(fd, EV_ABS, ABS_MT_SLOT, 0);
    emitEvent(fd, EV_ABS, ABS_MT_TRACKING_ID, trackingId);
    emitEvent(fd, EV_ABS, ABS_MT_POSITION_X, clampValue(x0, 0, config.xMax));
    emitEvent(fd, EV_ABS, ABS_MT_POSITION_Y, clampValue(y0, 0, config.yMax));
    emitEvent(fd, EV_ABS, ABS_MT_TOUCH_MAJOR, config.touchMajor);
    emitEvent(fd, EV_ABS, ABS_MT_WIDTH_MAJOR, config.widthMajor);
    emitEvent(fd, EV_KEY, BTN_TOUCH, 1);
    syncEvents(fd);

    const int totalSteps = std::max(1, steps);
    for (int index = 1; index <= totalSteps; ++index) {
        const double t = static_cast<double>(index) / static_cast<double>(totalSteps);
        const int xi = static_cast<int>(std::lround(x0 + (x1 - x0) * t));
        const int yi = static_cast<int>(std::lround(y0 + (y1 - y0) * t));
        emitEvent(fd, EV_ABS, ABS_MT_POSITION_X, clampValue(xi, 0, config.xMax));
        emitEvent(fd, EV_ABS, ABS_MT_POSITION_Y, clampValue(yi, 0, config.yMax));
        emitEvent(fd, EV_ABS, ABS_MT_TOUCH_MAJOR, config.touchMajor);
        emitEvent(fd, EV_ABS, ABS_MT_WIDTH_MAJOR, config.widthMajor);
        syncEvents(fd);
        if (stepDelayMs > 0) {
            QThread::msleep(static_cast<unsigned long>(stepDelayMs));
        }
    }

    emitEvent(fd, EV_KEY, BTN_TOUCH, 0);
    emitEvent(fd, EV_ABS, ABS_MT_TRACKING_ID, -1);
    syncEvents(fd);
    return true;
}

bool runSwipe(const QString &eventDevice,
              int x0,
              int y0,
              int x1,
              int y1,
              int steps,
              int stepDelayMs,
              const TouchConfig &config,
              QTextStream &out,
              QTextStream &err)
{
    const auto tryEventDevice = [&](const QString &requestedDevice) {
        const QString device = (requestedDevice == QStringLiteral("auto")) ? autoDetectEventDevice() : requestedDevice;
        qInfo() << "[AudbBridge/helper] swipe backend=evdev"
                << "device=" << device
                << "x0=" << x0
                << "y0=" << y0
                << "x1=" << x1
                << "y1=" << y1
                << "steps=" << steps
                << "stepDelayMs=" << stepDelayMs;
        const int fd = ::open(device.toUtf8().constData(), O_WRONLY);
        if (fd < 0) {
            err << "Failed to open " << device << ": " << strerror(errno) << '\n';
            return false;
        }
        performSwipe(fd, x0, y0, x1, y1, steps, stepDelayMs, config);
        ::close(fd);
        out << "swipe via " << device << '\n';
        return true;
    };

    if (!eventDevice.isEmpty()) {
        return tryEventDevice(eventDevice);
    }

    const int fd = openUinput();
    if (fd >= 0) {
        qInfo() << "[AudbBridge/helper] swipe backend=uinput"
                << "x0=" << x0
                << "y0=" << y0
                << "x1=" << x1
                << "y1=" << y1
                << "steps=" << steps
                << "stepDelayMs=" << stepDelayMs;
        if (!configureUinput(fd, config, err)) {
            destroyUinput(fd);
            return false;
        }

        performSwipe(fd, x0, y0, x1, y1, steps, stepDelayMs, config);
        QThread::msleep(kDefaultReleaseSettleMs);
        destroyUinput(fd);
        out << "swipe via uinput" << '\n';
        return true;
    }

    err << "Falling back to direct event device because /dev/uinput is unavailable: "
        << strerror(errno) << '\n';
    return tryEventDevice(QStringLiteral("auto"));
}

QRect gestureRectForDirection(const QString &direction, int orientation, const TouchConfig &config)
{
    const QSize visible = (orientation == kOrientationLandscape || orientation == kOrientationInvertedLandscape)
            ? QSize(config.yMax, config.xMax)
            : QSize(config.xMax, config.yMax);

    const int width = visible.width();
    const int height = visible.height();
    const int centerX = static_cast<int>(std::lround(width * kSwipeCenterX));
    const int centerY = static_cast<int>(std::lround(height * kSwipeCenterY));
    const int leftEdge = static_cast<int>(std::lround(width * kEdgeStartX));
    const int rightEdge = static_cast<int>(std::lround(width * (1.0 - kEdgeStartX)));
    const int topEdge = static_cast<int>(std::lround(height * (1.0 - kEdgeStartY)));
    const int bottomEdge = static_cast<int>(std::lround(height * kEdgeStartY));
    const int leftInner = static_cast<int>(std::lround(width * kEdgeEndX));
    const int rightInner = static_cast<int>(std::lround(width * (1.0 - kEdgeEndX)));
    const int topInner = static_cast<int>(std::lround(height * kEdgeEndY));
    const int bottomInner = static_cast<int>(std::lround(height * (1.0 - kEdgeEndY)));
    const int scrollTop = static_cast<int>(std::lround(height * kScrollEndY));
    const int scrollBottom = static_cast<int>(std::lround(height * kScrollStartY));
    const int longTopInner = static_cast<int>(std::lround(height * kLongEdgeEndY));
    const int longBottomInner = static_cast<int>(std::lround(height * (1.0 - kLongEdgeEndY)));

    if (direction == QStringLiteral("lr")) {
        return QRect(leftEdge, centerY, leftInner - leftEdge, 0);
    }
    if (direction == QStringLiteral("rl")) {
        return QRect(rightEdge, centerY, rightInner - rightEdge, 0);
    }
    if (direction == QStringLiteral("du")) {
        return QRect(centerX, scrollBottom, 0, scrollTop - scrollBottom);
    }
    if (direction == QStringLiteral("ud")) {
        return QRect(centerX, scrollTop, 0, scrollBottom - scrollTop);
    }
    if (direction == QStringLiteral("longdu")) {
        return QRect(centerX, bottomEdge, 0, longTopInner - bottomEdge);
    }
    return QRect(centerX, topEdge, 0, longBottomInner - topEdge);
}

bool runSystemShell(const QString &command, QTextStream &err)
{
    QProcess process;
    process.start(QStringLiteral("/bin/sh"), {QStringLiteral("-lc"), command});
    if (!process.waitForFinished(10000)) {
        process.kill();
        process.waitForFinished();
        err << "Timed out running: " << command << '\n';
        return false;
    }

    if (process.exitStatus() != QProcess::NormalExit || process.exitCode() != 0) {
        const QString stdErr = QString::fromUtf8(process.readAllStandardError()).trimmed();
        err << (stdErr.isEmpty() ? QStringLiteral("Command failed: %1").arg(command) : stdErr) << '\n';
        return false;
    }
    return true;
}

int handleProbe(QTextStream &out, QTextStream &err)
{
    const int fd = openUinput();
    if (fd >= 0) {
        ::close(fd);
        out << "uinput available" << '\n';
        return 0;
    }

    const QString fallback = autoDetectEventDevice();
    const int evdevFd = ::open(fallback.toUtf8().constData(), O_WRONLY);
    if (evdevFd >= 0) {
        ::close(evdevFd);
        out << "evdev available via " << fallback << '\n';
        return 0;
    }

    err << "Neither /dev/uinput nor writable touchscreen event device is available." << '\n';
    return 1;
}

int handleTap(QCommandLineParser &parser, QTextStream &out, QTextStream &err)
{
    TouchConfig config;
    config.xMax = parser.value(QStringLiteral("xmax")).toInt();
    config.yMax = parser.value(QStringLiteral("ymax")).toInt();
    config.slotMax = parser.value(QStringLiteral("slot-max")).toInt();
    config.touchMajor = parser.value(QStringLiteral("touch-major")).toInt();
    config.widthMajor = parser.value(QStringLiteral("width-major")).toInt();
    config.settleMs = parser.value(QStringLiteral("settle-ms")).toInt();

    const int orientation = parser.value(QStringLiteral("orientation")).toInt();
    const int durationMs = parser.value(QStringLiteral("duration-ms")).toInt();
    QPoint point(parser.value(QStringLiteral("x")).toInt(), parser.value(QStringLiteral("y")).toInt());
    point = transformCoordinates(point.x(), point.y(), orientation, config.xMax, config.yMax);
    qInfo() << "[AudbBridge/helper] handleTap"
            << "orientation=" << orientation
            << "transformed=" << point
            << "durationMs=" << durationMs
            << "eventDevice=" << parser.value(QStringLiteral("event-device"));
    return runTap(parser.value(QStringLiteral("event-device")),
                  point.x(),
                  point.y(),
                  durationMs,
                  config,
                  out,
                  err) ? 0 : 1;
}

int handleSwipe(QCommandLineParser &parser, QTextStream &out, QTextStream &err)
{
    TouchConfig config;
    config.xMax = parser.value(QStringLiteral("xmax")).toInt();
    config.yMax = parser.value(QStringLiteral("ymax")).toInt();
    config.slotMax = parser.value(QStringLiteral("slot-max")).toInt();
    config.touchMajor = parser.value(QStringLiteral("touch-major")).toInt();
    config.widthMajor = parser.value(QStringLiteral("width-major")).toInt();
    config.settleMs = parser.value(QStringLiteral("settle-ms")).toInt();

    const int orientation = parser.value(QStringLiteral("orientation")).toInt();
    int x0 = 0;
    int y0 = 0;
    int x1 = 0;
    int y1 = 0;
    const QString direction = parser.value(QStringLiteral("direction"));
    if (!direction.isEmpty()) {
        const QRect gesture = gestureRectForDirection(direction, orientation, config);
        qInfo() << "[AudbBridge/helper] swipe direction gesture"
                << "direction=" << direction
                << "orientation=" << orientation
                << "gesture=" << gesture;
        x0 = gesture.x();
        y0 = gesture.y();
        x1 = gesture.x() + gesture.width();
        y1 = gesture.y() + gesture.height();
    } else {
        x0 = parser.value(QStringLiteral("x1")).toInt();
        y0 = parser.value(QStringLiteral("y1")).toInt();
        x1 = parser.value(QStringLiteral("x2")).toInt();
        y1 = parser.value(QStringLiteral("y2")).toInt();
    }

    const QPoint start = transformCoordinates(x0, y0, orientation, config.xMax, config.yMax);
    const QPoint end = transformCoordinates(x1, y1, orientation, config.xMax, config.yMax);
    qInfo() << "[AudbBridge/helper] handleSwipe"
            << "orientation=" << orientation
            << "start=" << start
            << "end=" << end
            << "steps=" << parser.value(QStringLiteral("steps")).toInt()
            << "stepDelayMs=" << parser.value(QStringLiteral("step-delay-ms")).toInt()
            << "eventDevice=" << parser.value(QStringLiteral("event-device"));

    return runSwipe(parser.value(QStringLiteral("event-device")),
                    start.x(),
                    start.y(),
                    end.x(),
                    end.y(),
                    parser.value(QStringLiteral("steps")).toInt(),
                    parser.value(QStringLiteral("step-delay-ms")).toInt(),
                    config,
                    out,
                    err) ? 0 : 1;
}

int handleKey(QCommandLineParser &parser, QTextStream &out, QTextStream &err)
{
    const QString key = parser.value(QStringLiteral("key")).trimmed().toLower();
    TouchConfig config;
    config.xMax = parser.value(QStringLiteral("xmax")).toInt();
    config.yMax = parser.value(QStringLiteral("ymax")).toInt();
    const int orientation = parser.value(QStringLiteral("orientation")).toInt();

    if (key == QStringLiteral("power")) {
        return runSystemShell(QStringLiteral("gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_trigger_powerkey_event 0"), err) ? 0 : 1;
    }

    if (key == QStringLiteral("lock")) {
        return runSystemShell(QStringLiteral("gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_tklock_mode_change 'locked'"), err) ? 0 : 1;
    }

    if (key == QStringLiteral("unlock")) {
        const bool first = runSystemShell(QStringLiteral("gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_tklock_mode_change 'unlocked'"), err);
        const bool second = runSystemShell(QStringLiteral("gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_display_state_on"), err);
        return (first && second) ? 0 : 1;
    }

    if (key == QStringLiteral("volumeup") || key == QStringLiteral("vol+")
            || key == QStringLiteral("volumedown") || key == QStringLiteral("vol-")) {
        const int code = (key.startsWith(QStringLiteral("volumeu")) || key == QStringLiteral("vol+"))
                ? KEY_VOLUMEUP : KEY_VOLUMEDOWN;
        const int fd = ::open("/dev/input/event1", O_WRONLY);
        if (fd < 0) {
            err << "Failed to open /dev/input/event1: " << strerror(errno) << '\n';
            return 1;
        }
        for (int index = 0; index < 2; ++index) {
            emitEvent(fd, EV_KEY, code, 1);
            syncEvents(fd);
            QThread::msleep(50);
            emitEvent(fd, EV_KEY, code, 0);
            syncEvents(fd);
            QThread::msleep(100);
        }
        ::close(fd);
        out << "key " << key << '\n';
        return 0;
    }

    QString direction;
    if (key == QStringLiteral("home") || key == QStringLiteral("close")) {
        direction = QStringLiteral("du");
    } else if (key == QStringLiteral("back")) {
        direction = QStringLiteral("lr");
    } else if (key == QStringLiteral("menu")) {
        direction = QStringLiteral("ud");
    }

    if (!direction.isEmpty()) {
        const QRect gesture = gestureRectForDirection(direction, orientation, config);
        const QPoint start = transformCoordinates(gesture.x(),
                                                  gesture.y(),
                                                  orientation,
                                                  config.xMax,
                                                  config.yMax);
        const QPoint end = transformCoordinates(gesture.x() + gesture.width(),
                                                gesture.y() + gesture.height(),
                                                orientation,
                                                config.xMax,
                                                config.yMax);
        const QString eventDevice = parser.value(QStringLiteral("event-device"));

        return runSwipe(eventDevice,
                        start.x(),
                        start.y(),
                        end.x(),
                        end.y(),
                        kDefaultSwipeSteps,
                        kDefaultSwipeStepDelayMs,
                        config,
                        out,
                        err) ? 0 : 1;
    }

    err << "Unsupported key: " << key << '\n';
    return 1;
}

void addCommonOptions(QCommandLineParser &parser)
{
    parser.addOption({QStringLiteral("bridge-helper"), QStringLiteral("Internal helper action."), QStringLiteral("action")});
    parser.addOption({QStringLiteral("x"), QStringLiteral("Tap X coordinate."), QStringLiteral("x"), QString::number(kDefaultXMax / 2)});
    parser.addOption({QStringLiteral("y"), QStringLiteral("Tap Y coordinate."), QStringLiteral("y"), QString::number(kDefaultYMax / 2)});
    parser.addOption({QStringLiteral("x1"), QStringLiteral("Swipe start X."), QStringLiteral("x1"), QStringLiteral("0")});
    parser.addOption({QStringLiteral("y1"), QStringLiteral("Swipe start Y."), QStringLiteral("y1"), QStringLiteral("0")});
    parser.addOption({QStringLiteral("x2"), QStringLiteral("Swipe end X."), QStringLiteral("x2"), QStringLiteral("0")});
    parser.addOption({QStringLiteral("y2"), QStringLiteral("Swipe end Y."), QStringLiteral("y2"), QStringLiteral("0")});
    parser.addOption({QStringLiteral("direction"), QStringLiteral("Swipe direction: lr, rl, du, ud, longdu, longud."), QStringLiteral("direction")});
    parser.addOption({QStringLiteral("event-device"), QStringLiteral("Direct event device path or auto."), QStringLiteral("path")});
    parser.addOption({QStringLiteral("duration-ms"), QStringLiteral("Tap duration in milliseconds."), QStringLiteral("ms"), QString::number(kDefaultDownMs)});
    parser.addOption({QStringLiteral("steps"), QStringLiteral("Swipe move step count."), QStringLiteral("count"), QString::number(kDefaultSwipeSteps)});
    parser.addOption({QStringLiteral("step-delay-ms"), QStringLiteral("Swipe delay between steps."), QStringLiteral("ms"), QString::number(kDefaultSwipeStepDelayMs)});
    parser.addOption({QStringLiteral("orientation"), QStringLiteral("Screen orientation as Qt enum value."), QStringLiteral("value"), QString::number(kOrientationPortrait)});
    parser.addOption({QStringLiteral("xmax"), QStringLiteral("Touchscreen X max."), QStringLiteral("value"), QString::number(kDefaultXMax)});
    parser.addOption({QStringLiteral("ymax"), QStringLiteral("Touchscreen Y max."), QStringLiteral("value"), QString::number(kDefaultYMax)});
    parser.addOption({QStringLiteral("slot-max"), QStringLiteral("Touch slot max."), QStringLiteral("value"), QString::number(kDefaultSlotMax)});
    parser.addOption({QStringLiteral("touch-major"), QStringLiteral("Touch major value."), QStringLiteral("value"), QString::number(kDefaultTouchMajor)});
    parser.addOption({QStringLiteral("width-major"), QStringLiteral("Width major value."), QStringLiteral("value"), QString::number(kDefaultWidthMajor)});
    parser.addOption({QStringLiteral("settle-ms"), QStringLiteral("uinput settle time in ms."), QStringLiteral("value"), QString::number(kDefaultSettleMs)});
    parser.addOption({QStringLiteral("key"), QStringLiteral("Key name."), QStringLiteral("name")});
}

}

int HelperCli::run(const QStringList &arguments)
{
    QCommandLineParser parser;
    parser.setSingleDashWordOptionMode(QCommandLineParser::ParseAsLongOptions);
    addCommonOptions(parser);
    parser.process(arguments);

    QTextStream out(stdout);
    QTextStream err(stderr);

    const QString action = parser.value(QStringLiteral("bridge-helper")).trimmed().toLower();
    if (action.isEmpty()) {
        err << "Missing --bridge-helper action." << '\n';
        return 2;
    }

    if (action == QStringLiteral("probe")) {
        return handleProbe(out, err);
    }
    if (action == QStringLiteral("tap")) {
        return handleTap(parser, out, err);
    }
    if (action == QStringLiteral("swipe")) {
        return handleSwipe(parser, out, err);
    }
    if (action == QStringLiteral("key")) {
        return handleKey(parser, out, err);
    }

    err << "Unsupported helper action: " << action << '\n';
    return 2;
}
