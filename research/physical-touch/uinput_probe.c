#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <linux/input.h>
#include <linux/uinput.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

static int fd = -1, active, tracking = 100;
static int width, height, rotation;
static struct uinput_user_dev device;
static volatile sig_atomic_t stopping;

static void on_signal(int sig) { (void)sig; stopping = 1; }
static int event(unsigned type, unsigned code, int value) {
    struct input_event e = {0};
    e.type = type; e.code = code; e.value = value;
    return write(fd, &e, sizeof(e)) == sizeof(e) ? 0 : -1;
}
static void sleep_ms(int ms) {
    struct timespec ts = {ms / 1000, (ms % 1000) * 1000000L};
    while (!stopping && nanosleep(&ts, &ts) < 0 && errno == EINTR) {}
}
static int axis(unsigned code, int maximum) {
    device.absmax[code] = maximum;
    if (ioctl(fd, UI_SET_ABSBIT, code) < 0) return -1;
    return 0;
}
static int configure(void) {
    fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) return -1;
    if (ioctl(fd, UI_SET_EVBIT, EV_KEY) < 0 ||
        ioctl(fd, UI_SET_EVBIT, EV_ABS) < 0 ||
        ioctl(fd, UI_SET_KEYBIT, BTN_TOUCH) < 0 ||
        ioctl(fd, UI_SET_PROPBIT, INPUT_PROP_DIRECT) < 0 ||
        axis(ABS_MT_SLOT, 0) < 0 || axis(ABS_MT_TRACKING_ID, 65535) < 0 ||
        axis(ABS_MT_POSITION_X, width - 1) < 0 ||
        axis(ABS_MT_POSITION_Y, height - 1) < 0 ||
        axis(ABS_MT_TOUCH_MAJOR, 200) < 0 ||
        axis(ABS_MT_WIDTH_MAJOR, 200) < 0 ||
        axis(ABS_MT_PRESSURE, 200) < 0) return -1;
    snprintf(device.name, sizeof(device.name), "audb-persistent-touch-probe");
    device.id.bustype = BUS_VIRTUAL;
    if (write(fd, &device, sizeof(device)) != sizeof(device)) return -1;
    return ioctl(fd, UI_DEV_CREATE);
}
static int transform(int *x, int *y) {
    int sx = *x, sy = *y;
    int sw = (rotation == 90 || rotation == 270) ? height : width;
    int sh = (rotation == 90 || rotation == 270) ? width : height;
    if (sx < 0 || sy < 0 || sx >= sw || sy >= sh) return -1;
    if (rotation == 90) { *x = width - 1 - sy; *y = sx; }
    else if (rotation == 180) { *x = width - 1 - sx; *y = height - 1 - sy; }
    else if (rotation == 270) { *x = sy; *y = height - 1 - sx; }
    return 0;
}
static int position(int x, int y) {
    if (event(EV_ABS, ABS_MT_POSITION_X, x) < 0 ||
        event(EV_ABS, ABS_MT_POSITION_Y, y) < 0 ||
        event(EV_ABS, ABS_MT_TOUCH_MAJOR, 19) < 0 ||
        event(EV_ABS, ABS_MT_WIDTH_MAJOR, 19) < 0 ||
        event(EV_ABS, ABS_MT_PRESSURE, 50) < 0) return -1;
    return event(EV_SYN, SYN_REPORT, 0);
}
static int down(int x, int y) {
    active = 1;
    if (event(EV_KEY, BTN_TOUCH, 1) < 0 || event(EV_ABS, ABS_MT_SLOT, 0) < 0 ||
        event(EV_ABS, ABS_MT_TRACKING_ID, ++tracking) < 0) return -1;
    return position(x, y);
}
static int up(void) {
    if (!active) return 0;
    active = 0;
    int failed = 0;
    failed |= event(EV_KEY, BTN_TOUCH, 0);
    failed |= event(EV_ABS, ABS_MT_PRESSURE, 0);
    failed |= event(EV_ABS, ABS_MT_TRACKING_ID, -1);
    failed |= event(EV_SYN, SYN_REPORT, 0);
    return failed ? -1 : 0;
}
static void cleanup(void) {
    if (fd >= 0) { up(); ioctl(fd, UI_DEV_DESTROY); close(fd); fd = -1; }
}
int main(int argc, char **argv) {
    if (argc != 4) { fprintf(stderr, "usage: probe native_width native_height rotation\n"); return 2; }
    width = atoi(argv[1]); height = atoi(argv[2]); rotation = atoi(argv[3]);
    if (width < 2 || height < 2 || width > 16384 || height > 16384 ||
        (rotation != 0 && rotation != 90 && rotation != 180 && rotation != 270)) return 2;
    signal(SIGTERM, on_signal); signal(SIGINT, on_signal);
    atexit(cleanup);
    if (configure() < 0) { perror("uinput setup"); return 1; }
    sleep_ms(700);
    char sysname[128] = {0};
    ioctl(fd, UI_GET_SYSNAME(sizeof(sysname)), sysname);
    printf("{\"ready\":true,\"sysname\":\"%s\",\"nativeWidth\":%d,\"nativeHeight\":%d,\"rotation\":%d}\n", sysname, width, height, rotation);
    fflush(stdout);
    char line[256];
    while (!stopping) {
        struct pollfd in = {STDIN_FILENO, POLLIN, 0};
        int pr = poll(&in, 1, 100);
        if (pr < 0 && errno == EINTR) continue;
        if (pr < 0) return 1;
        if (!pr) continue;
        if (!fgets(line, sizeof(line), stdin)) break;
        if (!strncmp(line, "quit", 4)) break;
        errno = 0;
        int x1, y1, x2, y2, ms, rc = -1;
        if (sscanf(line, "tap %d %d %d", &x1, &y1, &ms) == 3 &&
            ms >= 1 && ms <= 3000 && transform(&x1, &y1) == 0) {
            rc = down(x1, y1);
            if (!rc) sleep_ms(ms);
            if (up() < 0) rc = -1;
        } else if (sscanf(line, "swipe %d %d %d %d %d", &x1, &y1, &x2, &y2, &ms) == 5 &&
                   ms >= 40 && ms <= 3000 && transform(&x1, &y1) == 0 && transform(&x2, &y2) == 0) {
            rc = down(x1, y1);
            for (int i = 1; !rc && !stopping && i <= 40; ++i) {
                sleep_ms(ms / 40);
                rc = position(x1 + (x2 - x1) * i / 40, y1 + (y2 - y1) * i / 40);
            }
            if (up() < 0) rc = -1;
        }
        printf("{\"ok\":%s}\n", rc ? "false" : "true");
        fflush(stdout);
        if (rc && errno) { perror("touch action"); return 1; }
    }
    return 0;
}
