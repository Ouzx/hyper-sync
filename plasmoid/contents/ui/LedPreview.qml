import QtQuick
import QtQuick.Effects
import org.kde.plasma.components as PlasmaComponents3
import org.kde.kirigami as Kirigami

// The strip around a wide screen, as the TUI draws it: live LED colors (every 5th numbered), their
// glow on the wall, and the effect and frame rate in the middle.
Item {
    id: preview

    property string text
    property string subText
    property color subColor: "#9aa3b5"

    readonly property int n: root.leds.length || 65
    readonly property var seg: root.segments(n)
    readonly property real gap: Kirigami.Units.smallSpacing  // screen edge to strip
    readonly property real thick: Math.max(2, Kirigami.Units.smallSpacing * 0.75)

    implicitHeight: Kirigami.Units.gridUnit * 5.5

    // Center of LED i just outside the screen, its length along the edge, and whether that edge
    // runs across. The strip starts bottom right and runs counter-clockwise (layout.rs).
    function place(i, out) {
        const m = screen, r = seg[0], t = seg[1], l = seg[2];
        if (i < r[1])
            return { x: m.x + m.width + gap + out, y: m.y + m.height * (1 - (i + 0.5) / r[1]), len: m.height / r[1], across: false };
        if (i < t[1])
            return { x: m.x + m.width * (1 - (i - t[0] + 0.5) / (t[1] - t[0])), y: m.y - gap - out, len: m.width / (t[1] - t[0]), across: true };
        return { x: m.x - gap - out, y: m.y + m.height * (i - l[0] + 0.5) / (l[1] - l[0]), len: m.height / (l[1] - l[0]), across: false };
    }

    function color(i) {
        return root.lit && i < root.leds.length ? root.leds[i] : Qt.alpha(Kirigami.Theme.textColor, 0.15);
    }

    // The light thrown on the wall: blocks around the strip, blurred.
    Item {
        id: glowSource
        anchors.fill: parent
        visible: false

        Repeater {
            model: preview.n
            Rectangle {
                readonly property var p: preview.place(index, Kirigami.Units.gridUnit * 0.4)
                width: p.across ? p.len * 1.2 : Kirigami.Units.gridUnit * 0.8
                height: p.across ? Kirigami.Units.gridUnit * 0.8 : p.len * 1.2
                x: p.x - width / 2
                y: p.y - height / 2
                color: preview.color(index)
                Behavior on color { ColorAnimation { duration: 125 } }
            }
        }
    }
    MultiEffect {
        source: glowSource
        anchors.fill: glowSource
        autoPaddingEnabled: true
        blurEnabled: true
        blurMax: 48
        blur: 1
        opacity: root.lit ? 0.9 : 0
        Behavior on opacity { NumberAnimation { duration: Kirigami.Units.longDuration } }
    }

    // The strip itself.
    Repeater {
        model: preview.n
        Rectangle {
            readonly property var p: preview.place(index, preview.thick / 2)
            width: p.across ? p.len * 0.75 : preview.thick
            height: p.across ? preview.thick : p.len * 0.75
            x: p.x - width / 2
            y: p.y - height / 2
            radius: preview.thick / 2
            color: preview.color(index)
            Behavior on color { ColorAnimation { duration: 125 } }
        }
    }

    // The screen (colors from the app icon, the same in light and dark themes).
    Rectangle {
        id: screen
        x: Kirigami.Units.gridUnit * 1.2
        y: Kirigami.Units.gridUnit * 1.1
        width: preview.width - 2 * x
        height: preview.height - y
        radius: Kirigami.Units.cornerRadius
        border.color: "#3d4659"
        border.width: 1
        gradient: Gradient {
            GradientStop { position: 0; color: "#1c2230" }
            GradientStop { position: 1; color: "#0c0f16" }
        }

        // Every 5th LED's number (and the last), just inside the edge it sits on.
        Repeater {
            model: preview.n
            PlasmaComponents3.Label {
                readonly property var p: preview.place(index, 0)
                readonly property real pad: Kirigami.Units.smallSpacing * 1.5
                visible: index % 5 === 0 || index === preview.n - 1
                text: index
                color: "#5c6577"
                font.pixelSize: Kirigami.Theme.smallFont.pixelSize > 0 ? Kirigami.Theme.smallFont.pixelSize : 9
                x: p.across ? p.x - screen.x - width / 2 : p.x < screen.x ? pad : screen.width - width - pad
                y: p.across ? pad / 2 : Math.max(pad / 2, Math.min(screen.height - height - pad / 2, p.y - screen.y - height / 2))
            }
        }

        Column {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 6
            PlasmaComponents3.Label {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                text: preview.text
                color: "#f0b35a"
                font.weight: Font.Bold
                font.capitalization: Font.AllUppercase
                font.letterSpacing: 1
            }
            PlasmaComponents3.Label {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                text: preview.subText
                visible: text !== ""
                color: preview.subColor
                font: Kirigami.Theme.smallFont
            }
        }
    }
}
