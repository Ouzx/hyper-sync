import QtQuick
import QtQuick.Effects
import org.kde.kirigami as Kirigami

// The tray icon: hyper-sync's app icon, greyed while the lights are off.
// Click: popup · middle-click: control panel (TUI) · right-click: menu (Plasma's).
MouseArea {
    id: compact

    property bool wasExpanded

    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    hoverEnabled: true
    onPressed: wasExpanded = root.expanded
    onClicked: mouse => {
        if (mouse.button === Qt.MiddleButton)
            root.openTui();
        else
            root.expanded = !wasExpanded;
    }

    Kirigami.Icon {
        anchors.fill: parent
        source: "hyper-sync"
        active: compact.containsMouse
        opacity: root.lit ? 1 : 0.6
        layer.enabled: !root.lit
        layer.effect: MultiEffect {
            saturation: -1
        }
    }
}
