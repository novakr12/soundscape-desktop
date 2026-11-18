import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtWebEngine
import io.github.novakr12.soundscape

ApplicationWindow {
    id: window

    property string server: app.server()
    property int frames: 0
    property int fps: 0
    property int memory: 0

    width: 1280
    height: 820
    visible: true
    title: "Soundscape (Qt preview)"
    color: "#111111"
    onFrameSwapped: frames += 1

    App {
        id: app
    }

    Timer {
        interval: 1000
        running: app.showFps()
        repeat: true
        onTriggered: {
            window.fps = window.frames;
            window.frames = 0;
            window.memory = app.memoryMb();
        }
    }

    WebEngineProfile {
        id: profile

        storageName: "soundscape"
        offTheRecord: false
        persistentCookiesPolicy: WebEngineProfile.ForcePersistentCookies
    }

    WebEngineView {
        id: web

        anchors.fill: parent
        visible: window.server !== ""
        profile: profile
        url: window.server
        settings.playbackRequiresUserGesture: false
        onLoadingChanged: function(info) {
            if (info.status === WebEngineView.LoadSucceededStatus && app.showFps())
                web.runJavaScript(app.frameCounter());
        }
        onNavigationRequested: function(request) {
            if (request.navigationType !== WebEngineNavigationRequest.LinkClickedNavigation)
                return;
            if (app.isOwnPage(request.url.toString()))
                return;
            Qt.openUrlExternally(request.url);
            if (typeof request.reject === "function")
                request.reject();
            else
                request.action = WebEngineNavigationRequest.IgnoreRequest;
        }
        onNewWindowRequested: function(request) {
            Qt.openUrlExternally(request.requestedUrl);
        }
    }

    Rectangle {
        visible: app.showFps()
        x: 6
        y: 6
        z: 10
        radius: 4
        color: "#c0000000"
        width: readout.implicitWidth + 12
        height: readout.implicitHeight + 4

        Text {
            id: readout

            anchors.centerIn: parent
            color: "#00ff66"
            font.family: "monospace"
            font.pixelSize: 12
            text: "Qt " + window.fps + " fps · " + window.memory + " MB"
        }
    }

    ColumnLayout {
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 420)
        visible: window.server === ""
        spacing: 12

        Label {
            Layout.fillWidth: true
            color: "#eeeeee"
            font.pixelSize: 22
            text: "Soundscape (Qt preview)"
        }

        Label {
            Layout.fillWidth: true
            color: "#aaaaaa"
            wrapMode: Text.WordWrap
            text: "Enter the address of your Soundscape server."
        }

        TextField {
            id: address

            Layout.fillWidth: true
            placeholderText: "music.example.com"
            onAccepted: open.clicked()
        }

        Button {
            id: open

            text: "Open"
            onClicked: {
                const saved = app.saveServer(address.text);
                if (saved === "")
                    problem.visible = true;
                else
                    window.server = saved;
            }
        }

        Label {
            id: problem

            visible: false
            color: "#ff6b6b"
            text: "That doesn't look like a server address."
        }
    }
}
