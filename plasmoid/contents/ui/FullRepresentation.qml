import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import QtCore
import org.kde.plasma.components as PlasmaComponents3
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami

// The TUI's layout: the strip preview on top, then controls and stats (left) beside the effect list
// (right); stacked when the popup is narrow. Controls the current effect doesn't use are hidden.
PlasmaExtras.Representation {
    id: full

    // The TUI's color presets.
    readonly property var colors: [
        ["Warm orange", "ff3300"], ["Red", "ff0000"], ["White", "ffffff"], ["Blue", "0099ff"],
        ["Green", "00ff88"], ["Magenta", "ff00ff"], ["Yellow", "ffff00"], ["Rainbow", "rainbow"]
    ]
    readonly property var effect: root.effects.find(e => e.id === root.st.effect) ?? { uses: {} }
    readonly property bool screenSync: root.st.effect === "screen" || root.st.effect === "screen_center"
    readonly property bool soundOn: (root.st.sound_mode ?? "off") !== "off"
    readonly property bool wide: width >= Kirigami.Units.gridUnit * 30
    readonly property real labelWidth: labelMetrics.width + Kirigami.Units.smallSpacing
    readonly property real valueWidth: valueMetrics.width

    // Off and the favorites, then every effect under its group; `header` starts a section.
    readonly property var effectRows: {
        const rows = [], favorites = root.effects.filter(e => e.id === "off" || e.favorite);
        favorites.forEach((e, k) => rows.push(Object.assign({ header: k === 0 ? "★ Favorites" : "" }, e)));
        root.effects.filter(e => e.id !== "off").forEach((e, k, all) =>
            rows.push(Object.assign({ header: k === 0 || all[k - 1].group !== e.group ? full.cap(e.group) : "" }, e)));
        return rows;
    }

    function cap(s) {
        return s.charAt(0).toUpperCase() + s.slice(1);
    }
    function pct(v) {
        return Math.round((v ?? 0) * 100) + "%";
    }
    function ms(v) {
        return v < 1 ? "off" : Math.round(v) + " ms";
    }
    function ampColor(a) {
        return a < 0.5 ? Kirigami.Theme.positiveTextColor : a < 0.9 ? Kirigami.Theme.neutralTextColor : Kirigami.Theme.negativeTextColor;
    }
    function uptime() {
        const up = Math.max(0, Math.floor(Date.now() / 1000) - (root.st.started ?? 0));
        return !root.st.started ? "?" : up >= 3600 ? `${Math.floor(up / 3600)}h ${Math.floor(up / 60) % 60}m` : `${Math.floor(up / 60)}m ${up % 60}s`;
    }

    TextMetrics {
        id: labelMetrics
        text: "Screen smoothing"
    }
    TextMetrics {
        id: valueMetrics
        text: "000 ms"
    }

    // A framed panel with a title, like the TUI's boxes.
    component Panel: Rectangle {
        default property alias content: body.data
        property alias title: heading.text

        Layout.fillWidth: true
        implicitHeight: heading.implicitHeight + body.implicitHeight + Kirigami.Units.largeSpacing * 2
        radius: Kirigami.Units.cornerRadius
        color: Qt.alpha(Kirigami.Theme.textColor, 0.03)
        border.color: Qt.alpha(Kirigami.Theme.textColor, 0.12)

        PlasmaComponents3.Label {
            id: heading
            x: Kirigami.Units.largeSpacing
            y: Kirigami.Units.smallSpacing * 1.5
            font: Kirigami.Theme.smallFont
            opacity: 0.6
        }
        ColumnLayout {
            id: body
            anchors {
                top: heading.bottom
                bottom: parent.bottom
                left: parent.left
                right: parent.right
                leftMargin: Kirigami.Units.largeSpacing
                rightMargin: Kirigami.Units.largeSpacing
                bottomMargin: Kirigami.Units.smallSpacing * 1.5
            }
            spacing: 0
        }
    }

    // One control: a label, the control, and its value on the right.
    component Row: RowLayout {
        default property alias control: slot.data
        property alias label: name.text
        property alias value: value.text

        Layout.fillWidth: true
        Layout.minimumHeight: Kirigami.Units.gridUnit * 1.6
        spacing: Kirigami.Units.largeSpacing

        PlasmaComponents3.Label {
            id: name
            Layout.preferredWidth: full.labelWidth
            elide: Text.ElideRight
            opacity: 0.7
        }
        RowLayout {
            id: slot
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
        }
        PlasmaComponents3.Label {
            id: value
            Layout.preferredWidth: full.valueWidth
            horizontalAlignment: Text.AlignRight
        }
    }

    component ValueSlider: PlasmaComponents3.Slider {
        required property string key
        Layout.fillWidth: true
        value: root.st[key] ?? 0
        onMoved: root.set(key, Math.round(value * 100) / 100)
    }

    // Segmented choice: one ToolButton per option, the current one checked.
    component Choice: Repeater {
        required property string key
        required property var options  // [[value, label], ...]
        model: options
        PlasmaComponents3.ToolButton {
            required property var modelData
            text: modelData[1]
            checked: root.st[key] === modelData[0]
            onClicked: root.set(key, modelData[0])
        }
    }

    component StatLabel: PlasmaComponents3.Label {
        Layout.preferredWidth: full.labelWidth
        opacity: 0.7
    }

    // A group of controls, like the TUI's "─ light" lines: a dim title, then a rule across the row.
    component Group: RowLayout {
        property alias text: heading.text
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.largeSpacing
        Layout.bottomMargin: Kirigami.Units.smallSpacing
        spacing: Kirigami.Units.largeSpacing

        PlasmaComponents3.Label {
            id: heading
            font: Kirigami.Theme.smallFont
            opacity: 0.6
        }
        Kirigami.Separator {
            Layout.fillWidth: true
        }
    }

    Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    Layout.minimumHeight: Kirigami.Units.gridUnit * 20
    Layout.preferredWidth: Kirigami.Units.gridUnit * 44
    Layout.preferredHeight: Kirigami.Units.gridUnit * 32

    header: PlasmaExtras.PlasmoidHeading {
        visible: root.daemonUp
        contentItem: RowLayout {
            PlasmaComponents3.Switch {
                text: "Lights"
                checked: root.lit
                onToggled: root.toggle()
            }
            Item {
                Layout.fillWidth: true
            }
            PlasmaComponents3.ToolButton {
                visible: full.screenSync
                icon.name: "monitor-symbolic"
                text: "Choose Screen…"
                onClicked: root.run("hyper-sync ctl reselect-screen")
            }
            PlasmaComponents3.ToolButton {
                icon.name: "view-refresh"
                text: "Restart"
                onClicked: root.run("hyper-sync ctl restart")
                PlasmaComponents3.ToolTip.text: "Restart the effect"
                PlasmaComponents3.ToolTip.visible: hovered
            }
            PlasmaComponents3.ToolButton {
                icon.name: "utilities-terminal"
                text: "Control Panel"
                onClicked: root.openTui()
                PlasmaComponents3.ToolTip.text: "Open the terminal control panel (TUI) · middle-click the tray icon"
                PlasmaComponents3.ToolTip.visible: hovered
            }
            PlasmaComponents3.ToolButton {
                icon.name: "document-edit"
                text: "Edit Settings File…"
                display: QQC2.AbstractButton.IconOnly
                onClicked: root.openSettings()
                PlasmaComponents3.ToolTip.text: text
                PlasmaComponents3.ToolTip.visible: hovered
            }
        }
    }

    contentItem: Item {
        PlasmaExtras.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 4
            visible: !root.daemonUp
            iconName: "hyper-sync"
            text: "hyper-sync isn't running"
            explanation: "Start it to control the LED strip."
            helpfulAction: QQC2.Action {
                text: "Start hyper-sync"
                icon.name: "media-playback-start"
                onTriggered: root.start()
            }
        }

        ColumnLayout {
            anchors.fill: parent
            visible: root.daemonUp
            spacing: Kirigami.Units.largeSpacing

            LedPreview {
                Layout.fillWidth: true
                text: root.lit ? root.label(root.st.effect) : "Lights off"
                subText: !root.lit ? "" : `${Math.round(root.st.actual_fps ?? 0)}/${root.st.fps} fps · ` + (root.st.serial_ok ? "strip connected" : "strip unplugged")
                subColor: root.st.serial_ok ? "#7ee787" : "#ff7b72"
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                visible: !!root.st.last_error
                type: Kirigami.MessageType.Error
                text: root.st.last_error ?? ""
            }

            GridLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                columns: full.wide ? 2 : 1
                columnSpacing: Kirigami.Units.largeSpacing
                rowSpacing: Kirigami.Units.largeSpacing

                PlasmaComponents3.ScrollView {
                    id: scroll
                    Layout.fillWidth: true
                    Layout.fillHeight: full.wide
                    Layout.preferredHeight: full.wide ? -1 : Math.min(left.implicitHeight, full.height * 0.55)
                    contentWidth: availableWidth
                    PlasmaComponents3.ScrollBar.horizontal.policy: PlasmaComponents3.ScrollBar.AlwaysOff

                    ColumnLayout {
                        id: left
                        width: scroll.availableWidth
                        spacing: Kirigami.Units.largeSpacing

                        Panel {
                            title: "Controls"

                            Group {
                                text: "Light"
                                Layout.topMargin: Kirigami.Units.smallSpacing
                            }
                            Row {
                                label: "Brightness"
                                value: full.pct(root.st.brightness)
                                ValueSlider {
                                    key: "brightness"
                                    from: 0
                                    to: 1
                                }
                            }
                            Row {
                                label: "Frame rate"
                                Choice {
                                    key: "fps"
                                    options: [[24, "24"], [30, "30"], [45, "45"], [55, "55"]]
                                }
                                Item {
                                    Layout.fillWidth: true
                                }
                            }
                            Group {
                                visible: !!(full.effect.uses.speed || full.effect.uses.power || full.effect.uses.color) || root.st.effect === "candle"
                                text: "Effect"
                            }
                            Row {
                                visible: full.effect.uses.speed ?? false
                                label: "Speed"
                                value: (root.st.speed ?? 1).toFixed(1) + "×"
                                ValueSlider {
                                    key: "speed"
                                    from: 0.1
                                    to: 5
                                }
                            }
                            Row {
                                visible: full.effect.uses.power ?? false
                                label: "Strength"
                                value: full.pct(root.st.power)
                                ValueSlider {
                                    key: "power"
                                    from: 0
                                    to: 1
                                }
                            }
                            Row {
                                visible: full.effect.uses.color ?? false
                                label: "Color"
                                value: root.st.color === "rainbow" ? "rainbow" : `#${root.st.color}`
                                Repeater {
                                    model: full.colors
                                    QQC2.AbstractButton {
                                        id: swatch
                                        required property var modelData
                                        readonly property bool selected: modelData[1] === root.st.color
                                        text: modelData[0]  // read out by screen readers; the swatch draws no label
                                        implicitWidth: Kirigami.Units.iconSizes.smallMedium
                                        implicitHeight: Kirigami.Units.iconSizes.smallMedium
                                        hoverEnabled: true
                                        onClicked: root.set("color", modelData[1])
                                        PlasmaComponents3.ToolTip.text: text
                                        PlasmaComponents3.ToolTip.visible: hovered

                                        background: Rectangle {
                                            radius: width / 2
                                            color: swatch.modelData[1] === "rainbow" ? "transparent" : "#" + swatch.modelData[1]
                                            border.width: swatch.selected ? 2 : 1
                                            border.color: swatch.selected ? Kirigami.Theme.highlightColor : Qt.alpha(Kirigami.Theme.textColor, swatch.hovered ? 0.6 : 0.2)
                                            Rectangle {
                                                anchors.fill: parent
                                                anchors.margins: swatch.selected ? 3 : 0
                                                radius: width / 2
                                                visible: swatch.modelData[1] === "rainbow" || swatch.selected
                                                color: swatch.modelData[1] === "rainbow" ? "transparent" : "#" + swatch.modelData[1]
                                                gradient: swatch.modelData[1] === "rainbow" ? rainbow : null
                                            }
                                        }
                                    }
                                }
                                Item {
                                    Layout.fillWidth: true
                                }
                            }
                            Row {
                                visible: full.effect.uses.color ?? false
                                label: "Custom"
                                // The TUI's color dial: any hue at full strength.
                                PlasmaComponents3.Slider {
                                    id: hue
                                    Layout.fillWidth: true
                                    from: 0
                                    to: 1
                                    value: (root.st.color ?? "").length === 6 ? Math.max(0, Qt.color("#" + root.st.color).hsvHue) : 0
                                    onMoved: root.set("color", Qt.hsva(value, 1, 1, 1).toString().slice(1))
                                    background: Rectangle {
                                        x: hue.leftPadding
                                        y: hue.topPadding + hue.availableHeight / 2 - height / 2
                                        width: hue.availableWidth
                                        height: Kirigami.Units.smallSpacing * 1.5
                                        radius: height / 2
                                        gradient: rainbow
                                    }
                                }
                            }
                            Row {
                                visible: root.st.effect === "candle"
                                label: "Warmth"
                                value: full.pct(root.st.warmth)
                                ValueSlider {
                                    key: "warmth"
                                    from: 0
                                    to: 1
                                }
                            }
                            Group {
                                visible: full.screenSync
                                text: "Screen sync"
                            }
                            Row {
                                visible: full.screenSync
                                label: "Gamma"
                                value: (root.st.gamma ?? 1).toFixed(1)
                                ValueSlider {
                                    key: "gamma"
                                    from: 1
                                    to: 3
                                }
                            }
                            Row {
                                visible: full.screenSync
                                label: "Smoothing"
                                value: full.ms(600 * (root.st.screen_smoothing ?? 0) ** 2)
                                ValueSlider {
                                    key: "screen_smoothing"
                                    from: 0
                                    to: 1
                                }
                            }
                            Group {
                                text: "Sound"
                            }
                            Row {
                                label: "Mode"
                                Choice {
                                    key: "sound_mode"
                                    options: [["off", "Off"], ["level", "Level"], ["balance", "Balance"]]
                                }
                                Item {
                                    Layout.fillWidth: true
                                }
                            }
                            Row {
                                visible: full.soundOn
                                label: "Reaction"
                                value: full.pct(root.st.reactivity)
                                // One knob for both: reactivity and sensitivity always move together here.
                                ValueSlider {
                                    key: "reactivity"
                                    from: 0
                                    to: 1
                                    onMoved: root.set("sensitivity", Math.round(value * 100) / 100)
                                }
                            }
                            Row {
                                visible: full.soundOn
                                label: "Smoothing"
                                value: full.ms(50 + 550 * (root.st.audio_smoothing ?? 0))
                                ValueSlider {
                                    key: "audio_smoothing"
                                    from: 0
                                    to: 1
                                }
                            }
                            Row {
                                visible: full.soundOn || root.st.effect === "sound_viz"
                                label: "Audio"
                                value: full.pct(root.st.audio_level)
                                // The spectrum the daemon hears, low to high.
                                Repeater {
                                    model: root.st.spectrum ?? []
                                    Rectangle {
                                        required property real modelData
                                        Layout.fillWidth: true
                                        Layout.alignment: Qt.AlignBottom
                                        Layout.bottomMargin: Kirigami.Units.smallSpacing
                                        implicitHeight: Math.max(2, modelData * Kirigami.Units.gridUnit)
                                        radius: 1
                                        color: Kirigami.Theme.highlightColor
                                        Behavior on implicitHeight { NumberAnimation { duration: 125 } }
                                    }
                                }
                            }
                        }

                        Panel {
                            title: "Stats"

                            GridLayout {
                                Layout.fillWidth: true
                                columns: 2
                                columnSpacing: Kirigami.Units.largeSpacing
                                rowSpacing: Kirigami.Units.smallSpacing

                                StatLabel {
                                    text: "Daemon"
                                }
                                PlasmaComponents3.Label {
                                    Layout.fillWidth: true
                                    text: `${Math.round(root.st.actual_fps ?? 0)}/${root.st.fps} fps · up ${full.uptime()}`
                                }
                                StatLabel {
                                    text: "Current"
                                }
                                PlasmaComponents3.Label {
                                    Layout.fillWidth: true
                                    textFormat: Text.StyledText
                                    text: `<font color="${full.ampColor(root.amps)}">≈${root.amps.toFixed(2)} A</font> now · `
                                        + `<font color="${full.ampColor(root.peakAmps)}">${root.peakAmps.toFixed(2)} A</font> peak (est.)`
                                }
                                StatLabel {
                                    text: "Strip"
                                }
                                PlasmaComponents3.Label {
                                    readonly property var seg: root.segments(root.leds.length || 65)
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                    text: `${root.leds.length || 65} LEDs (${seg[0][1]}/${seg[1][1] - seg[1][0]}/${seg[2][1] - seg[2][0]})` + (root.st.port ? ` · ${root.st.port}` : "")
                                }
                                StatLabel {
                                    visible: full.screenSync && root.st.width > 0
                                    text: "Screen"
                                }
                                PlasmaComponents3.Label {
                                    visible: full.screenSync && root.st.width > 0
                                    Layout.fillWidth: true
                                    text: `capturing ${root.st.width}×${root.st.height}`
                                }
                                StatLabel {
                                    text: "Config"
                                }
                                PlasmaComponents3.Label {
                                    readonly property string home: StandardPaths.writableLocation(StandardPaths.HomeLocation).toString().replace("file://", "")
                                    readonly property string path: StandardPaths.writableLocation(StandardPaths.GenericConfigLocation).toString().replace("file://", "") + "/hyper-sync/config.toml"
                                    Layout.fillWidth: true
                                    elide: Text.ElideMiddle
                                    text: path.startsWith(home) ? "~" + path.slice(home.length) : path
                                }
                            }
                        }
                    }
                }

                Panel {
                    title: "Effects"
                    Layout.fillHeight: true
                    Layout.fillWidth: !full.wide
                    Layout.preferredWidth: full.wide ? Math.max(Kirigami.Units.gridUnit * 14, full.width * 0.36) : -1

                    PlasmaComponents3.ScrollView {
                        id: effectScroll
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        contentWidth: availableWidth
                        PlasmaComponents3.ScrollBar.horizontal.policy: PlasmaComponents3.ScrollBar.AlwaysOff

                        ColumnLayout {
                            width: effectScroll.availableWidth
                            spacing: 0

                            Repeater {
                                model: full.effectRows
                                ColumnLayout {
                                    id: entry
                                    required property var modelData
                                    Layout.fillWidth: true
                                    spacing: 0

                                    PlasmaComponents3.Label {
                                        visible: entry.modelData.header !== ""
                                        Layout.topMargin: Kirigami.Units.smallSpacing
                                        text: entry.modelData.header
                                        font: Kirigami.Theme.smallFont
                                        opacity: 0.6
                                    }
                                    PlasmaComponents3.ItemDelegate {
                                        id: effectRow
                                        readonly property var e: entry.modelData
                                        Layout.fillWidth: true
                                        topPadding: 1
                                        bottomPadding: 1
                                        highlighted: e.id === root.st.effect
                                        onClicked: root.set("effect", e.id)

                                        contentItem: RowLayout {
                                            spacing: Kirigami.Units.smallSpacing
                                            PlasmaComponents3.Label {
                                                Layout.preferredWidth: numberMetrics.width
                                                horizontalAlignment: Text.AlignRight
                                                text: effectRow.e.n
                                                opacity: 0.5
                                                TextMetrics {
                                                    id: numberMetrics
                                                    text: "00"
                                                }
                                            }
                                            PlasmaComponents3.Label {
                                                Layout.fillWidth: true
                                                elide: Text.ElideRight
                                                text: effectRow.e.label
                                                font.weight: effectRow.highlighted ? Font.DemiBold : Font.Normal
                                            }
                                            PlasmaComponents3.Label {
                                                visible: effectRow.e.hyperion
                                                text: "H"
                                                opacity: 0.5
                                            }
                                            PlasmaComponents3.ToolButton {
                                                visible: effectRow.e.id !== "off"
                                                opacity: effectRow.e.favorite || effectRow.hovered ? 1 : 0
                                                implicitWidth: Kirigami.Units.iconSizes.small + Kirigami.Units.smallSpacing * 2
                                                implicitHeight: implicitWidth
                                                icon.name: effectRow.e.favorite ? "starred-symbolic" : "non-starred-symbolic"
                                                icon.width: Kirigami.Units.iconSizes.small
                                                icon.height: Kirigami.Units.iconSizes.small
                                                onClicked: root.toggleFavorite(effectRow.e.id)
                                                PlasmaComponents3.ToolTip.text: effectRow.e.favorite ? "Unpin from favorites" : "Pin to favorites"
                                                PlasmaComponents3.ToolTip.visible: hovered
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Gradient {
        id: rainbow
        orientation: Gradient.Horizontal
        GradientStop { position: 0; color: "#ff0000" }
        GradientStop { position: 0.17; color: "#ffff00" }
        GradientStop { position: 0.33; color: "#00ff00" }
        GradientStop { position: 0.5; color: "#00ffff" }
        GradientStop { position: 0.67; color: "#0000ff" }
        GradientStop { position: 0.83; color: "#ff00ff" }
        GradientStop { position: 1; color: "#ff0000" }
    }
}
