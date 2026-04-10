import QtQuick 2.0
import Sailfish.Silica 1.0
import "helpers"

ApplicationWindow {
    objectName: "applicationWindow"
    initialPage: Qt.resolvedUrl("pages/MainPage.qml")
    cover: Qt.resolvedUrl("cover/DefaultCoverPage.qml")
    allowedOrientations: defaultAllowedOrientations

    ScreenshotGrabber {
        id: screenshotGrabber
    }

    Component.onCompleted: bridgeService.setScreenshotGrabber(screenshotGrabber)
}
