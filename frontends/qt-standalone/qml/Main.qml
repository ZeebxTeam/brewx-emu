import QtCore
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts
import brewx

ApplicationWindow {
    id: window
    width: 1200
    height: 680
    visible: true
    title: "brewx"

    // Path given on the command line (`brewx-qt <path>`): opened right away.
    property string initialPath: ""
    // `--screenshot <png>`: saves the window after `screenshotDelay` ms and exits.
    property string screenshotPath: ""
    property int screenshotDelay: 300

    Inspector { id: inspector }

    // The ROM folder survives between sessions (~/.config/brewx/brewx.conf on Linux).
    Settings {
        id: settings
        property string romsFolder: ""
    }

    function scanRoms() {
        games.model = settings.romsFolder.length > 0
                ? JSON.parse(inspector.scan(settings.romsFolder))
                : []
    }

    function setRomsFolder(folder) {
        settings.romsFolder = folder
        scanRoms()
    }

    FolderDialog {
        id: folderDialog
        title: "Pasta de ROMs"
        currentFolder: settings.romsFolder.length > 0 ? "file://" + settings.romsFolder : ""
        onAccepted: window.setRomsFolder(decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }

    function inspect() {
        if (pathField.text.length === 0) {
            report.text = "Informe um .mod, .mif ou uma pasta, ou arraste-o para a janela."
            return
        }
        report.text = inspector.inspect(pathField.text, versionBox.currentIndex, resolutionBox.currentIndex)
    }

    function openGame() {
        inspect()
        screen.open(pathField.text, versionBox.currentIndex, resolutionBox.currentIndex)
        screen.forceActiveFocus()
    }

    Component.onCompleted: {
        scanRoms()
        pathField.text = initialPath
        if (initialPath.length > 0)
            openGame()
        else
            inspect()
        if (screenshotPath.length > 0)
            shotTimer.start()
    }

    Timer {
        id: shotTimer
        interval: window.screenshotDelay
        // The window's `contentItem` has no QML engine and refuses `grabToImage`; the `Pane` has one.
        onTriggered: page.grabToImage(result => {
            result.saveToFile(window.screenshotPath)
            Qt.quit()
        })
    }

    // The frontend loop: ~60 steps per second while a game is open.
    Timer {
        interval: 16
        repeat: true
        running: screen.running
        onTriggered: screen.tick()
    }

    DropArea {
        anchors.fill: parent
        onDropped: (drop) => {
            if (drop.hasUrls) {
                pathField.text = decodeURIComponent(drop.urls[0].toString().replace(/^file:\/\//, ""))
                window.openGame()
            }
        }
    }

    Pane {
        id: page
        anchors.fill: parent

        ColumnLayout {
            anchors.fill: parent
            spacing: 8

            RowLayout {
                Layout.fillWidth: true
                TextField {
                    id: pathField
                    Layout.fillWidth: true
                    placeholderText: "Caminho do jogo (.mod, .mif ou pasta)"
                    onAccepted: window.openGame()
                }
                ComboBox {
                    id: versionBox
                    Layout.preferredWidth: 140
                    model: inspector.versionOptions()
                    onActivated: window.inspect()
                }
                ComboBox {
                    id: resolutionBox
                    Layout.preferredWidth: 130
                    model: inspector.resolutionOptions()
                    onActivated: window.inspect()
                }
                Button {
                    text: "Abrir"
                    onClicked: window.openGame()
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 8

                ColumnLayout {
                    Layout.preferredWidth: 240
                    Layout.maximumWidth: 240
                    Layout.fillHeight: true
                    spacing: 4

                    RowLayout {
                        Layout.fillWidth: true
                        TextField {
                            id: romsField
                            Layout.fillWidth: true
                            placeholderText: "Pasta de ROMs"
                            text: settings.romsFolder
                            onAccepted: window.setRomsFolder(text)
                        }
                        Button {
                            text: "…"
                            implicitWidth: 32
                            onClicked: folderDialog.open()
                        }
                    }

                    Label {
                        Layout.fillWidth: true
                        font.pointSize: 8
                        opacity: 0.7
                        text: games.count + " jogo(s)"
                    }

                    ListView {
                        id: games
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        model: []
                        ScrollBar.vertical: ScrollBar {}

                        delegate: ItemDelegate {
                            required property var modelData
                            width: ListView.view.width
                            highlighted: pathField.text === modelData.path
                            contentItem: RowLayout {
                                spacing: 8
                                Image {
                                    Layout.preferredWidth: 32
                                    Layout.preferredHeight: 32
                                    source: modelData.icon
                                    fillMode: Image.PreserveAspectFit
                                    smooth: false
                                    asynchronous: true
                                    // Sem ícone: a inicial do título.
                                    Label {
                                        anchors.centerIn: parent
                                        visible: parent.status !== Image.Ready
                                        text: modelData.title.charAt(0).toUpperCase()
                                        font.bold: true
                                    }
                                }
                                Label {
                                    Layout.fillWidth: true
                                    text: modelData.title
                                    elide: Text.ElideRight
                                }
                            }
                            onClicked: {
                                pathField.text = modelData.path
                                window.openGame()
                            }
                        }
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    color: "black"

                    GameScreen {
                        id: screen
                        anchors.fill: parent
                        focus: true
                        Keys.onPressed: (event) => {
                            if (!event.isAutoRepeat && screen.keyEvent(event.key, true))
                                event.accepted = true
                        }
                        Keys.onReleased: (event) => {
                            if (!event.isAutoRepeat && screen.keyEvent(event.key, false))
                                event.accepted = true
                        }
                        TapHandler { onTapped: screen.forceActiveFocus() }
                    }
                }

                ScrollView {
                    Layout.preferredWidth: 320
                    Layout.fillHeight: true
                    TextArea {
                        id: report
                        readOnly: true
                        wrapMode: TextEdit.NoWrap
                        font.family: "monospace"
                        font.pointSize: 8
                    }
                }
            }

            Label {
                Layout.fillWidth: true
                elide: Text.ElideRight
                text: screen.status.length > 0
                      ? screen.status + "   ·   setas, Enter = OK, Backspace = voltar, F1/F2 = soft keys, 0-9 * #"
                      : "Nenhum jogo aberto"
            }
        }
    }
}
