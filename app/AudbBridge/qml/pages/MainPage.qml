import QtQuick 2.0
import Sailfish.Silica 1.0

Page {
    objectName: "mainPage"
    allowedOrientations: Orientation.All

    function storePassword() {
        if (bridgeService.setPasswordForStorage(passwordField.text)) {
            passwordField.text = ""
        }
    }

    PageHeader {
        id: pageHeader
        objectName: "pageHeader"
        title: qsTr("AudbBridge")
        extraContent.children: [
            IconButton {
                objectName: "aboutButton"
                icon.source: "image://theme/icon-m-about"
                anchors.verticalCenter: parent.verticalCenter

                onClicked: pageStack.push(Qt.resolvedUrl("AboutPage.qml"))
            }
        ]
    }

    SilicaFlickable {
        anchors.fill: parent
        anchors.topMargin: pageHeader.height
        contentHeight: contentColumn.height + Theme.paddingLarge * 2

        Column {
            id: contentColumn
            width: parent.width - Theme.horizontalPageMargin * 2
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: Theme.paddingLarge

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                color: Theme.secondaryHighlightColor
                text: qsTr("Minimal Aurora bridge app for audb privileged input actions.")
            }

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: qsTr("Password status: %1").arg(bridgeService.passwordStatus)
            }

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                color: Theme.secondaryColor
                text: bridgeService.statusMessage
            }

            TextField {
                id: passwordField
                width: parent.width
                label: qsTr("devel-su password")
                placeholderText: qsTr("Enter password")
                echoMode: TextInput.Password
                inputMethodHints: Qt.ImhSensitiveData | Qt.ImhNoPredictiveText | Qt.ImhNoAutoUppercase
                EnterKey.enabled: text.length > 0
                EnterKey.iconSource: "image://theme/icon-m-enter-next"
                EnterKey.onClicked: storePassword()
            }

            Button {
                id: setPasswordButton
                width: parent.width
                text: qsTr("Set devel-su password")
                onClicked: storePassword()
            }

            Button {
                width: parent.width
                text: qsTr("Clear stored password")
                onClicked: {
                    bridgeService.clearStoredPassword()
                    passwordField.text = ""
                }
            }

            Button {
                width: parent.width
                text: qsTr("Run self-test")
                onClicked: bridgeService.selfTest()
            }

            Button {
                width: parent.width
                text: qsTr("Test tap")
                onClicked: bridgeService.runTapTest()
            }

            Button {
                width: parent.width
                text: qsTr("Test swipe up")
                onClicked: bridgeService.runSwipeUpTest()
            }

            Button {
                width: parent.width
                text: qsTr("Test screenshot")
                onClicked: {
                    var path = bridgeService.screenshot("/home/defaultuser/Pictures/Screenshots/test_audb.png")
                    console.log("Screenshot saved to:", path)
                }
            }
        }
    }
}
