/*
 * hyper-sync's System Tray widget: a popup with a live strip preview and the everyday controls.
 * It drives the daemon through `hyper-sync ctl`, since a widget can't open a Unix socket.
 */
import QtQuick
import QtCore
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasma5support as P5Support

PlasmoidItem {
    id: root

    // The daemon's last status ({} while it's down) and its LEDs as colors, scaled up by the
    // brightness so a dim setting still shows its hues.
    property var st: ({})
    property var leds: []
    property bool daemonUp: false
    readonly property bool lit: daemonUp && st.effect !== "off"
    // Estimated LED current (20 mA per channel at full, as the TUI counts it), now and the most
    // seen since the widget loaded.
    property real amps: 0
    property real peakAmps: 0
    // [{id, label, group, n, favorite, hyperion, uses: {speed, power, color}}] from `ctl effects`.
    property var effects: []
    // What the switch and a middle-click turn back on.
    property string lastOn: "screen"
    // Changes not sent yet, and a moment after each one in which polls are ignored, so a reply
    // that left before the change can't flick the controls back.
    property var pending: ({})
    property double holdUntil: 0

    // plasmashell's PATH may lack ~/.local/bin, where scripts/build.sh installs hyper-sync.
    function run(cmd) {
        exec.connectSource('export PATH="$HOME/.local/bin:$PATH"; ' + cmd);
    }

    function label(id) {
        const e = effects.find(e => e.id === id);
        return e ? e.label : (id || "");
    }

    // Right, top and left LED ranges, split as layout.rs does.
    function segments(n) {
        const r = Math.floor(n * 17 / 65), t = Math.floor(n * 31 / 65);
        return [[0, r], [r, r + t], [r + t, n]];
    }

    // Status fields whose `ctl set` flag has another name.
    readonly property var flags: ({ effect: "mode", screen_smoothing: "smoothing" })

    function set(key, value) {
        st = Object.assign({}, st, { [key]: value });
        pending[key] = value;
        holdUntil = Date.now() + 600;
        if (key === "effect" && value !== "off")
            lastOn = value;
        sendTimer.start();
    }

    function toggle() {
        set("effect", lit ? "off" : lastOn);
    }

    function toggleFavorite(id) {
        effects = effects.map(e => e.id === id ? Object.assign({}, e, { favorite: !e.favorite }) : e);
        run(`hyper-sync ctl favorite '${id}'; hyper-sync ctl effects`);
    }

    function openTui() {
        run('kstart -- konsole -e "$(command -v hyper-sync)" tui');
    }

    function openSettings() {
        Qt.openUrlExternally(StandardPaths.writableLocation(StandardPaths.GenericConfigLocation) + "/hyper-sync/config.toml");
    }

    function start() {
        run('systemctl --user start hyper-sync.service 2>/dev/null || kstart -- "$(command -v hyper-sync)" daemon');
    }

    function readStatus(stdout) {
        if (Date.now() < holdUntil)
            return;
        let s;
        try {
            s = JSON.parse(stdout).status;
        } catch (e) {}
        daemonUp = !!s;
        if (!s) {
            st = {};
            leds = [];
            return;
        }
        st = s;
        if (s.effect !== "off")
            lastOn = s.effect;
        const k = 255 * Math.max(s.brightness, 0.1), out = [];
        let sum = 0;
        for (let i = 0; i + 6 <= s.leds.length; i += 6) {
            const b = j => parseInt(s.leds.substr(i + j, 2), 16);
            sum += b(0) + b(2) + b(4);
            out.push(Qt.rgba(Math.min(1, b(0) / k), Math.min(1, b(2) / k), Math.min(1, b(4) / k), 1));
        }
        leds = out;
        amps = sum / 255 * 0.02;
        peakAmps = Math.max(peakAmps, amps);
    }

    function readEffects(stdout) {
        effects = stdout.trim().split("\n").filter(l => l).map((l, n) => {
            const [id, label, group, list] = l.split("\t"), f = (list || "").split(",");
            const has = flag => f.includes(flag);
            return { id, label, group, n, favorite: has("favorite"), hyperion: has("hyperion"),
                uses: { speed: has("speed"), power: has("power"), color: has("color") } };
        });
    }

    toolTipMainText: Plasmoid.title
    toolTipSubText: (!daemonUp ? "Not running" : lit ? `${label(st.effect)} · ${Math.round(st.brightness * 100)}%` : "Lights off")
        + "\nMiddle-click: control panel"

    compactRepresentation: CompactRepresentation {}
    fullRepresentation: FullRepresentation {}

    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: "Lights On"
            icon.name: "brightness-high"
            checkable: true
            checked: root.lit
            enabled: root.daemonUp
            onTriggered: root.toggle()
        },
        PlasmaCore.Action {
            text: "Open Control Panel"
            icon.name: "utilities-terminal"
            onTriggered: root.openTui()
        },
        PlasmaCore.Action {
            text: "Edit Settings File…"
            icon.name: "document-edit"
            onTriggered: root.openSettings()
        },
        PlasmaCore.Action {
            text: "Restart Effect"
            icon.name: "view-refresh"
            visible: root.daemonUp
            onTriggered: root.run("hyper-sync ctl restart")
        },
        PlasmaCore.Action {
            text: "Quit hyper-sync"
            icon.name: "application-exit"
            visible: root.daemonUp
            onTriggered: root.run("hyper-sync ctl quit")
        }
    ]

    onExpandedChanged: if (root.expanded) run("hyper-sync ctl effects")  // favorites may have changed
    Component.onCompleted: run("hyper-sync ctl effects")

    // Live preview while the popup is open, otherwise just enough for the icon and tooltip.
    Timer {
        interval: root.expanded && root.daemonUp ? 125 : 2000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.run("hyper-sync ctl status --json")
    }

    // Sends what changed at most every 100 ms, so dragging a slider doesn't start a process per pixel.
    Timer {
        id: sendTimer
        interval: 100
        onTriggered: {
            const flags = Object.keys(root.pending).map(k => `--${(root.flags[k] ?? k).replace("_", "-")} '${root.pending[k]}'`);
            root.pending = {};
            root.run("hyper-sync ctl set " + flags.join(" "));
        }
    }

    P5Support.DataSource {
        id: exec
        engine: "executable"
        onNewData: (source, data) => {
            disconnectSource(source);
            if (source.endsWith("ctl status --json"))
                root.readStatus(data.stdout);
            else if (source.endsWith("ctl effects"))
                root.readEffects(data.stdout);
        }
    }
}
