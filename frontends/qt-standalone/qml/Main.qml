import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import brewx

ApplicationWindow {
    id: window
    width: 720
    height: 520
    visible: true
    title: "brewx"

    // Path given on the command line (`brewx-qt <path>`).
    property string initialPath: ""
    // `--screenshot <png>`: saves the window after the first frame and exits.
    property string screenshotPath: ""

    Inspector { id: inspector }

    function refresh() {
        if (pathField.text.length === 0) {
            report.text = "Informe um .mod, .mif ou uma pasta, ou arraste-o para a janela."
            return
        }
        report.text = inspector.inspect(pathField.text, versionBox.currentIndex, resolutionBox.currentIndex)
    }

    Component.onCompleted: {
        pathField.text = initialPath
        refresh()
        if (screenshotPath.length > 0)
            shotTimer.start()
    }

    Timer {
        id: shotTimer
        interval: 300
        // The window's `contentItem` has no QML engine and refuses `grabToImage`; the `Pane` has one.
        onTriggered: page.grabToImage(result => {
            result.saveToFile(window.screenshotPath)
            Qt.quit()
        })
    }

    DropArea {
        anchors.fill: parent
        onDropped: (drop) => {
            if (drop.hasUrls) {
                pathField.text = decodeURIComponent(drop.urls[0].toString().replace(/^file:\/\//, ""))
                window.refresh()
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
                    onAccepted: window.refresh()
                }
                Button {
                    text: "Inspecionar"
                    onClicked: window.refresh()
                }
            }

            GridLayout {
                columns: 2
                Label { text: "Versão do BREW" }
                ComboBox {
                    id: versionBox
                    Layout.preferredWidth: 180
                    model: inspector.versionOptions()
                    onActivated: window.refresh()
                }
                Label { text: "Resolução da tela" }
                ComboBox {
                    id: resolutionBox
                    Layout.preferredWidth: 180
                    model: inspector.resolutionOptions()
                    onActivated: window.refresh()
                }
            }

            ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                TextArea {
                    id: report
                    readOnly: true
                    wrapMode: TextEdit.NoWrap
                    font.family: "monospace"
                }
            }
        }
    }
}
