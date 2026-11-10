"""Record the TUI (in a headless tmux) and the LED colors from an isolated hyper-sync daemon.

The daemon gets its own config dir and socket and `driver = "none"`, so a running daemon and the
LED strip are never touched. Needs a release build, tmux and Python 3. Then run render.py.

    cargo build --release --features full && python3 scripts/demo/record.py tui gallery
"""
import json, os, shutil, socket, subprocess, sys, time

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HERE = os.path.join(REPO, "target", "demo")  # recordings and scratch config
BIN = os.path.join(REPO, "target", "release", "hyper-sync")
CFG = os.path.join(HERE, "cfg")
RUN = "/run/user/%d/hsdemo" % os.getuid()  # short: unix socket paths max out at 108 bytes
SOCK = os.path.join(RUN, "hyper-sync.sock")
ENV = dict(os.environ, XDG_CONFIG_HOME=CFG, XDG_RUNTIME_DIR=RUN)
TMUX = ["tmux", "-L", "hsdemo", "-f", os.path.join(HERE, "tmux.conf")]
COLS, ROWS = 100, 32


def ipc(req):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(1)
    s.connect(SOCK)
    s.sendall((json.dumps(req) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    s.close()
    return json.loads(buf)


def leds():
    hexs = ipc({"cmd": "status"})["status"]["leds"]
    return [int(hexs[i:i + 2], 16) for i in range(0, len(hexs), 2)]


def tmux(*args):
    return subprocess.run(TMUX + list(args), capture_output=True, text=True, check=True).stdout


def setup():
    shutil.rmtree(CFG, ignore_errors=True)
    os.makedirs(os.path.join(CFG, "hyper-sync"))
    os.makedirs(RUN, exist_ok=True)
    cfg = open(os.path.join(REPO, "config", "runtime.toml")).read()
    cfg = cfg.replace('driver = "skydimo"', 'driver = "none"')
    cfg = cfg.replace('mode = "screen"', 'mode = "flow"')
    cfg = cfg.replace("brightness = 0.15", "brightness = 0.55")
    cfg = cfg.replace("fps = 30 ", "fps = 55 ")
    assert 'driver = "none"' in cfg and 'mode = "flow"' in cfg and "fps = 55" in cfg
    open(os.path.join(CFG, "hyper-sync", "config.toml"), "w").write(cfg)
    if "tui" not in sys.argv:  # the TUI recording pins it with `f`
        open(os.path.join(CFG, "hyper-sync", "favorites"), "w").write("knight_rider\n")
    open(os.path.join(HERE, "tmux.conf"), "w").write("set -g status off\nset -g default-terminal tmux-256color\n")


def main():
    setup()
    assert not os.path.exists(SOCK)
    daemon = subprocess.Popen([BIN, "daemon", "--no-tray"], env=ENV, stderr=open(os.path.join(HERE, "daemon.log"), "w"))
    try:
        for _ in range(50):
            if os.path.exists(SOCK):
                break
            time.sleep(0.1)
        st = ipc({"cmd": "status"})["status"]
        assert st["effect"] == "flow", st["effect"]  # our isolated daemon, not the real one
        time.sleep(1)

        tmux("new-session", "-d", "-s", "demo", "-x", str(COLS), "-y", str(ROWS),
             "-e", "XDG_CONFIG_HOME=" + CFG, "-e", "XDG_RUNTIME_DIR=" + RUN, BIN + " tui")
        time.sleep(1.5)

        if "tui" in sys.argv:
            record_tui()
        if "gallery" in sys.argv:
            record_gallery()
        tmux("send-keys", "-t", "demo", "q")
    finally:
        subprocess.run(TMUX + ["kill-server"], capture_output=True)
        try:
            ipc({"cmd": "quit"})
        except OSError:
            pass
        try:
            daemon.wait(5)
        except subprocess.TimeoutExpired:
            daemon.kill()
        shutil.rmtree(RUN, ignore_errors=True)


# (seconds, keys...) — one send-keys call each; digits are typed one by one.
TIMELINE = [
    (2.0, "Down"),          # swirl
    (4.2, "Down"),          # double swirl
    (4.5, "Down"),          # wave
    (6.8, "8"),             # aurora
    (9.0, "w"), (9.35, "w"), (9.7, "w"), (10.05, "w"), (10.4, "w"),
    (11.8, "6"),            # fire
    (14.0, "4"),            # solid
    (15.4, "Right"), (16.4, "Right"), (17.4, "Right"),
    (18.6, "2"), (18.75, "6"),  # knight rider
    (20.8, "f"),            # pin as favorite
    (22.6, "3"), (22.75, "8"),  # x-mas
    (24.8, "?"),            # help
    (27.6, "Escape"),
    (28.2, "7"),            # lava
]
DURATION = 31.0
FPS = 20


def record_tui():
    frames, i = [], 0
    t0 = time.monotonic()
    while True:
        t = time.monotonic() - t0
        if t >= DURATION:
            break
        while i < len(TIMELINE) and TIMELINE[i][0] <= t:
            tmux("send-keys", "-t", "demo", TIMELINE[i][1])
            i += 1
        frames.append({"t": t, "ansi": tmux("capture-pane", "-p", "-e", "-N", "-t", "demo"), "leds": leds()})
        time.sleep(max(0, (len(frames) / FPS) - (time.monotonic() - t0)))
    with open(os.path.join(HERE, "tui.jsonl"), "w") as f:
        for fr in frames:
            f.write(json.dumps(fr) + "\n")
    print("tui frames", len(frames))


GALLERY = ["aurora", "fire", "lava", "flow", "mood_blobs", "knight_rider", "police", "xmas", "color_waves", "plasma"]


def record_gallery():
    ipc({"cmd": "patch", "brightness": 1.0, "color": "rainbow"})
    out = {}
    for mode in GALLERY:
        ipc({"cmd": "patch", "mode": mode})
        time.sleep(2.0)
        samples = []
        for _ in range(12):
            samples.append(leds())
            time.sleep(0.12)
        out[mode] = max(samples, key=sum)
    # A clean TUI still (no overlay) for the README screenshot, on aurora.
    ipc({"cmd": "patch", "mode": "aurora", "brightness": 0.6})
    time.sleep(2.0)
    out["_tui"] = tmux("capture-pane", "-p", "-e", "-N", "-t", "demo")
    json.dump(out, open(os.path.join(HERE, "gallery.json"), "w"))
    print("gallery", list(out))


if __name__ == "__main__":
    main()
