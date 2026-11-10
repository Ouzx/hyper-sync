use std::io::stdout;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::daemon::{daemon_running, ipc_request, DaemonStatus, IpcRequest, Patch};
use crate::effects::solid::{hsv_to_rgb, is_rainbow_color, parse_color, rgb_to_hex, rgb_to_hsv};
use crate::layout::segment_bounds;

/// Effects in display order: (id, label, group, ported from Hyperion.ng). The index is the effect's
/// number key (Off is 0 and always pinned on top); groups become headers in the effects panel.
const MODES: &[(&str, &str, &str, bool)] = &[
    ("off", "Off", "screen & sound", false),
    ("screen", "Screen Sync", "screen & sound", false),
    ("screen_center", "Screen Center", "screen & sound", false),
    ("sound_viz", "Sound Viz", "screen & sound", false),
    ("solid", "Solid", "calm", false),
    ("candle", "Candle", "calm", false),
    ("fire", "Fire", "calm", false),
    ("lava", "Lava", "calm", false),
    ("aurora", "Aurora", "calm", false),
    ("pulse", "Pulse", "calm", false),
    ("breath", "Breath", "calm", true),
    ("cinema_brighten", "Cinema Brighten", "calm", true),
    ("cinema_dim", "Cinema Dim", "calm", true),
    ("rainbow_mood", "Rainbow Mood", "calm", true),
    ("mood_blobs", "Mood Blobs", "calm", true),
    ("sea_waves", "Sea Waves", "calm", true),
    ("color_waves", "Waves with Color", "calm", true),
    ("plasma", "Plasma", "calm", true),
    ("hyperion_fire", "Fire Animation", "calm", true),
    ("flow", "Rainbow Flow", "moving", false),
    ("swirl", "Swirl", "moving", true),
    ("double_swirl", "Double Swirl", "moving", true),
    ("wave", "Wave", "moving", false),
    ("chase", "Chase", "moving", false),
    ("snake", "Snake", "moving", true),
    ("scanner", "Scanner", "moving", false),
    ("knight_rider", "Knight Rider", "moving", true),
    ("juggler", "Juggler", "moving", true),
    ("collision", "Collision", "moving", true),
    ("traces", "Color Traces", "moving", true),
    ("trails", "Trails", "moving", true),
    ("police", "Police Lights", "moving", true),
    ("police_solid", "Police Solid", "moving", true),
    ("matrix", "Matrix", "moving", true),
    ("sparkle", "Sparkle", "sparkly", false),
    ("sparks", "Sparks", "sparkly", true),
    ("random", "Random", "sparkly", true),
    ("lightning", "Lightning", "sparkly", false),
    ("xmas", "X-Mas", "sparkly", true),
    ("lights", "Lights", "sparkly", true),
    ("segment", "Segment", "tools", false),
    ("light_clock", "Light Clock", "tools", true),
    ("flags", "Flags DE/SE", "tools", true),
    ("led_test", "LED Test", "tools", true),
    ("led_test_seq", "LED Test Sequence", "tools", true),
    ("notify_blue", "Notify Blue", "tools", true),
];
const SOUND_MODES: &[&str] = &["off", "level", "balance"];
const COLOR_PRESETS: &[&str] = &[
    "ff3300", "ff0000", "ffffff", "0099ff", "00ff88", "ff00ff", "ffff00", "rainbow",
];
// ponytail: one custom slot — overwrite, never stack custom2
const COLOR_SLOT_COUNT: usize = COLOR_PRESETS.len() + 1;
const CUSTOM_COLOR_IDX: usize = COLOR_PRESETS.len();
const DEFAULT_CUSTOM_HEX: &str = "ff6600";
const HUE_STEP: f32 = 1.0 / 72.0;
// 201-byte frames at 115200 baud (the only rate the Skydimo accepts) cap out near 57 fps.
const FPS_PRESETS: &[u32] = &[24, 30, 45, 55];
const SPEED_MIN: f32 = 0.1;
const SPEED_MAX: f32 = 5.0;
/// Status poll — also the strip preview's frame rate.
const POLL: Duration = Duration::from_millis(66);
/// Arrow keys browse; the effect under the cursor starts once you pause this long.
const BROWSE_SETTLE: Duration = Duration::from_millis(350);
/// After "1" or "2", wait this long for a second digit (effects 10–21).
const DIGIT_WAIT: Duration = Duration::from_millis(700);

struct ColorDial {
    hue: f32,
    prev_color: String,
    prev_custom_hex: String,
}

struct UiState {
    /// Last status from the daemon.
    st: DaemonStatus,
    connected: bool,
    color_idx: usize,
    custom_hex: String,
    color_dial: Option<ColorDial>,
    help: bool,
    /// Stats & config in place of the controls (small terminals; tall ones show both).
    info: bool,
    /// Position in the effects panel: favorites first, then every effect.
    cursor: usize,
    favorites: Vec<usize>,
    /// First visible line of the favorites and of the full list (kept between frames so the
    /// lists don't jump).
    scroll: [std::cell::Cell<usize>; 2],
    browse_since: Option<Instant>,
    digits: String,
    digits_since: Instant,
    /// The config file as last read (port, monitor, layout for the info panel).
    cfg: Option<crate::config::RuntimeConfig>,
    /// Estimated LED current now and the highest seen this session, in amps.
    amps: f32,
    peak_amps: f32,
}

pub fn run() -> anyhow::Result<()> {
    ensure_daemon()?;

    let quit = Arc::new(AtomicBool::new(false));
    let quit_signal = Arc::clone(&quit);
    ctrlc::set_handler(move || {
        quit_signal.store(true, Ordering::Relaxed);
    })
    .context("ctrl-c handler")?;

    enable_raw_mode().context("raw mode")?;
    stdout().execute(EnterAlternateScreen).context("alt screen")?;

    let result = tui_loop(quit);

    stdout().execute(LeaveAlternateScreen).ok();
    disable_raw_mode().ok();
    result
}

fn ensure_daemon() -> anyhow::Result<()> {
    if daemon_running() {
        return Ok(());
    }
    std::process::Command::new(std::env::current_exe().context("exe path")?)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn daemon")?;
    for _ in 0..50 {
        if daemon_running() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("daemon failed to start")
}

fn tui_loop(quit: Arc<AtomicBool>) -> anyhow::Result<()> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?;
    let mut state = UiState {
        st: DaemonStatus::default(),
        connected: false,
        color_idx: COLOR_PRESETS.len() - 1,
        custom_hex: DEFAULT_CUSTOM_HEX.into(),
        color_dial: None,
        help: false,
        info: false,
        cursor: 0,
        favorites: load_favorites(),
        scroll: Default::default(),
        browse_since: None,
        digits: String::new(),
        digits_since: Instant::now(),
        cfg: crate::config::RuntimeConfig::load(&crate::config::runtime_config_path()).ok(),
        amps: 0.0,
        peak_amps: 0.0,
    };
    poll_status(&mut state);
    state.cursor = list_pos(&state, mode_index(&state.st.effect).unwrap_or(0));
    let mut last_poll = Instant::now();

    while !quit.load(Ordering::Relaxed) {
        terminal.draw(|f| draw_ui(f, &state))?;

        // Sleep until a key arrives or the next poll is due — no busy redraw.
        // One key per redraw, so a held key shows every step instead of freezing the screen.
        if event::poll(POLL.saturating_sub(last_poll.elapsed()))? {
            if let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? {
                let overlay = state.help || state.color_dial.is_some();
                if kind != KeyEventKind::Release && code == KeyCode::Char('e') && !overlay {
                    edit_config(&mut terminal)?;
                    state.cfg = crate::config::RuntimeConfig::load(&crate::config::runtime_config_path()).ok();
                } else if kind != KeyEventKind::Release && handle_key(&mut state, code, modifiers) {
                    return Ok(());
                }
            }
        }
        if state.browse_since.is_some_and(|t| t.elapsed() >= BROWSE_SETTLE) {
            state.browse_since = None;
            let idx = cursor_mode(&state);
            select_mode(&mut state, idx);
        }
        if !state.digits.is_empty() && state.digits_since.elapsed() >= DIGIT_WAIT {
            apply_digits(&mut state);
        }
        if last_poll.elapsed() >= POLL {
            poll_status(&mut state);
            last_poll = Instant::now();
        }
    }
    Ok(())
}

/// Returns true to quit.
fn handle_key(state: &mut UiState, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }
    if state.color_dial.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => close_color_dial(state, false),
            KeyCode::Enter => close_color_dial(state, true),
            KeyCode::Left => nudge_color_dial(state, -1),
            KeyCode::Right => nudge_color_dial(state, 1),
            _ => {}
        }
        return false;
    }
    if state.help {
        state.help = false;
        return false;
    }
    let st = state.st.clone();
    match code {
        KeyCode::Char('q') | KeyCode::Esc if !state.digits.is_empty() => state.digits.clear(),
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('?') => state.help = true,
        KeyCode::Up | KeyCode::Down => {
            let dir = if code == KeyCode::Up { -1 } else { 1 };
            let len = state.favorites.len() + MODES.len();
            state.cursor = (state.cursor as i32 + dir).rem_euclid(len as i32) as usize;
            state.browse_since = Some(Instant::now());
        }
        KeyCode::Enter => {
            let idx = cursor_mode(state);
            select_mode(state, idx);
        }
        // Favorite: pin the effect under the cursor to the top and start it; again to unpin.
        KeyCode::Char('f' | 'F') if cursor_mode(state) != 0 => {
            let idx = cursor_mode(state);
            if let Some(k) = state.favorites.iter().position(|&f| f == idx) {
                state.favorites.remove(k);
            } else {
                state.favorites.push(idx);
                select_mode(state, idx);
            }
            state.cursor = list_pos(state, idx);
            state.browse_since = None;
            save_favorites(&state.favorites);
        }
        KeyCode::Char('i' | 'I') => state.info = !state.info,
        KeyCode::Char(c) if c.is_ascii_digit() => {
            state.digits.push(c);
            state.digits_since = Instant::now();
            // Apply as soon as no longer number could start with what was typed (0 is Off at once).
            if state.digits == "0" || state.digits.parse::<usize>().unwrap_or(0) * 10 > MODES.len() - 1 {
                apply_digits(state);
            }
        }
        KeyCode::Char('w' | 'W') => send(state, Patch { brightness: Some((st.brightness + 0.05).min(1.0)), ..Default::default() }),
        KeyCode::Char('s' | 'S') => send(state, Patch { brightness: Some((st.brightness - 0.05).max(0.0)), ..Default::default() }),
        KeyCode::Char('a' | 'A') if uses_speed(&st.effect) => {
            send(state, Patch { speed: Some((st.speed - 0.1).clamp(SPEED_MIN, SPEED_MAX)), ..Default::default() })
        }
        KeyCode::Char('d' | 'D') if uses_speed(&st.effect) => {
            send(state, Patch { speed: Some((st.speed + 0.1).clamp(SPEED_MIN, SPEED_MAX)), ..Default::default() })
        }
        KeyCode::Char('-') | KeyCode::Char('=') | KeyCode::Char('+') => {
            let dir = if code == KeyCode::Char('-') { -1 } else { 1 };
            let next = (fps_index(st.fps) as i32 + dir).rem_euclid(FPS_PRESETS.len() as i32) as usize;
            send(state, Patch { fps: Some(FPS_PRESETS[next]), ..Default::default() });
        }
        KeyCode::Left => cycle_color(state, -1),
        KeyCode::Right => cycle_color(state, 1),
        KeyCode::Char('c' | 'C') => open_color_dial(state),
        KeyCode::Char('z' | 'Z') | KeyCode::Char('x' | 'X') if uses_power(&st.effect) => {
            let d = if matches!(code, KeyCode::Char('z' | 'Z')) { -0.05 } else { 0.05 };
            send(state, Patch { power: Some((st.power + d).clamp(0.0, 1.0)), ..Default::default() });
        }
        KeyCode::Char('{') | KeyCode::Char('}') if is_screen_sync(&st.effect) => {
            let d = if code == KeyCode::Char('{') { -0.05 } else { 0.05 };
            send(state, Patch { smoothing: Some((st.screen_smoothing + d).clamp(0.0, 1.0)), ..Default::default() });
        }
        KeyCode::Char(',') | KeyCode::Char('.') if sound_on(&st) || st.effect == "sound_viz" => {
            let d = if code == KeyCode::Char(',') { -0.05 } else { 0.05 };
            send(state, Patch { audio_smoothing: Some((st.audio_smoothing + d).clamp(0.0, 1.0)), ..Default::default() });
        }
        KeyCode::Char('[') | KeyCode::Char(']') => {
            let dir = if code == KeyCode::Char('[') { -1.0 } else { 1.0 };
            if st.effect == "candle" {
                send(state, Patch { warmth: Some((st.warmth + 0.05 * dir).clamp(0.0, 1.0)), ..Default::default() });
            } else if is_screen_sync(&st.effect) {
                send(state, Patch { gamma: Some((st.gamma + 0.1 * dir).clamp(1.0, 3.0)), ..Default::default() });
            }
        }
        KeyCode::Tab => {
            let i = SOUND_MODES.iter().position(|m| *m == st.sound_mode).unwrap_or(0);
            let next = SOUND_MODES[(i + 1) % SOUND_MODES.len()].to_string();
            send(state, Patch { sound_mode: Some(next), ..Default::default() });
        }
        KeyCode::Char('j' | 'J') if sound_on(&st) => {
            send(state, Patch { reactivity: Some((st.reactivity - 0.05).max(0.0)), ..Default::default() })
        }
        KeyCode::Char('k' | 'K') if sound_on(&st) => {
            send(state, Patch { reactivity: Some((st.reactivity + 0.05).min(1.0)), ..Default::default() })
        }
        KeyCode::Char('h' | 'H') if sound_on(&st) => {
            send(state, Patch { sensitivity: Some((st.sensitivity - 0.05).max(0.0)), ..Default::default() })
        }
        KeyCode::Char('l' | 'L') if sound_on(&st) => {
            send(state, Patch { sensitivity: Some((st.sensitivity + 0.05).min(1.0)), ..Default::default() })
        }
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            ipc_request(&IpcRequest::Restart).ok();
        }
        KeyCode::Char('R') => {
            ipc_request(&IpcRequest::ReselectScreen).ok();
        }
        _ => {}
    }
    false
}

fn poll_status(state: &mut UiState) {
    match ipc_request(&IpcRequest::Status) {
        Ok(resp) => update_status(state, resp.status),
        Err(_) => state.connected = false,
    }
}

/// Send a change; the reply carries the new status, so the screen updates without waiting for a poll.
fn send(state: &mut UiState, patch: Patch) {
    match ipc_request(&IpcRequest::Patch(patch)) {
        Ok(resp) => update_status(state, resp.status),
        Err(_) => state.connected = false,
    }
}

fn update_status(state: &mut UiState, status: Option<DaemonStatus>) {
    let Some(mut st) = status else { return };
    if state.color_dial.is_some() {
        st.color = std::mem::take(&mut state.st.color);
    }
    if state.st.effect != st.effect && state.browse_since.is_none() {
        if let Some(i) = mode_index(&st.effect) {
            state.cursor = list_pos(state, i);
        }
    }
    // 20 mA per channel at full is the usual WS2812-style figure; an estimate, not a measurement.
    let leds = st.leds.as_bytes().chunks(2).filter_map(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
    state.amps = leds.map(|v| f32::from(v) / 255.0 * 0.02).sum();
    state.peak_amps = state.peak_amps.max(state.amps);
    state.st = st;
    state.connected = true;
    if state.color_dial.is_none() {
        if let Some(i) = COLOR_PRESETS.iter().position(|c| *c == state.st.color) {
            state.color_idx = i;
        } else if parse_color(&state.st.color).is_ok() {
            state.custom_hex = state.st.color.clone();
            state.color_idx = CUSTOM_COLOR_IDX;
        }
    }
}

fn select_mode(state: &mut UiState, idx: usize) {
    state.browse_since = None;
    if MODES[idx].0 != state.st.effect {
        // Set it first, so the reply doesn't move the cursor out of the favorites.
        state.st.effect = MODES[idx].0.into();
        send(state, Patch { mode: Some(MODES[idx].0.into()), ..Default::default() });
    }
}

/// The effect (index into MODES) under the cursor.
fn cursor_mode(state: &UiState) -> usize {
    match state.cursor {
        0 => 0,
        c if c <= state.favorites.len() => state.favorites[c - 1],
        c => (c - state.favorites.len()).min(MODES.len() - 1),
    }
}

/// Where effect `idx` sits in the effects panel: Off (always first), the favorites, then the rest.
fn list_pos(state: &UiState, idx: usize) -> usize {
    if idx == 0 {
        0
    } else {
        state.favorites.len() + idx
    }
}

fn favorites_path() -> std::path::PathBuf {
    crate::config::config_dir().join("favorites")
}

fn load_favorites() -> Vec<usize> {
    let text = std::fs::read_to_string(favorites_path()).unwrap_or_default();
    text.lines().filter_map(|l| mode_index(l.trim())).filter(|&i| i != 0).collect()
}

fn save_favorites(favorites: &[usize]) {
    let text: String = favorites.iter().map(|&i| format!("{}\n", MODES[i].0)).collect();
    let _ = std::fs::write(favorites_path(), text);
}

/// Open config.toml in $VISUAL/$EDITOR (the TUI steps aside meanwhile), or the desktop's editor.
/// The daemon applies the saved file live.
fn edit_config(terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>) -> anyhow::Result<()> {
    let path = crate::config::runtime_config_path();
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_default();
    let mut words = editor.split_whitespace();
    if let Some(program) = words.next() {
        stdout().execute(LeaveAlternateScreen)?;
        disable_raw_mode()?;
        let _ = std::process::Command::new(program).args(words).arg(&path).status();
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        terminal.clear()?;
    } else {
        let _ = std::process::Command::new("xdg-open").arg(&path).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
    Ok(())
}

fn apply_digits(state: &mut UiState) {
    let pick = state.digits.parse::<usize>().ok();
    state.digits.clear();
    if let Some(idx) = pick.filter(|&i| i < MODES.len()) {
        state.cursor = list_pos(state, idx);
        select_mode(state, idx);
    }
}

fn mode_index(mode: &str) -> Option<usize> {
    MODES.iter().position(|m| m.0 == mode)
}

fn fps_index(fps: u32) -> usize {
    (0..FPS_PRESETS.len()).min_by_key(|&i| FPS_PRESETS[i].abs_diff(fps)).unwrap_or(0)
}

fn sound_on(st: &DaemonStatus) -> bool {
    st.sound_mode != "off"
}

fn is_screen_sync(mode: &str) -> bool {
    mode == "screen" || mode == "screen_center"
}

fn uses_speed(effect: &str) -> bool {
    !matches!(effect, "off" | "solid" | "sound_viz" | "light_clock") && !is_screen_sync(effect)
}

/// Power only shapes our own effects; the Hyperion ports stay as their scripts draw them.
fn uses_power(effect: &str) -> bool {
    matches!(effect, "candle" | "fire" | "lava" | "aurora" | "pulse" | "wave" | "chase" | "scanner" | "sparkle" | "lightning" | "segment")
}

fn uses_accent_color(effect: &str) -> bool {
    matches!(
        effect,
        "solid" | "candle" | "lava" | "aurora" | "pulse" | "wave" | "chase" | "scanner" | "sparkle" | "lightning" | "segment" | "sound_viz"
            | "mood_blobs" | "swirl" | "snake" | "knight_rider" | "sparks" | "trails"
    )
}

fn cycle_color(state: &mut UiState, dir: i32) {
    if !uses_accent_color(&state.st.effect) {
        return;
    }
    state.color_idx = (state.color_idx as i32 + dir).rem_euclid(COLOR_SLOT_COUNT as i32) as usize;
    let color = if state.color_idx == CUSTOM_COLOR_IDX {
        state.custom_hex.clone()
    } else {
        COLOR_PRESETS[state.color_idx].into()
    };
    patch_color(state, &color);
}

fn patch_color(state: &mut UiState, color: &str) {
    state.st.color = color.to_string();
    send(state, Patch { color: Some(color.to_string()), ..Default::default() });
}

fn dial_start_hue(state: &UiState) -> f32 {
    // vivid LED colors: full sat/value; take hue from current hex or saved custom
    let hex = if parse_color(&state.st.color).is_ok() {
        state.st.color.as_str()
    } else {
        state.custom_hex.as_str()
    };
    match parse_color(hex) {
        Ok(rgb) => {
            let (h, s, _) = rgb_to_hsv(rgb);
            if s < 0.15 {
                // near-white/gray — start on a warm vivid hue instead
                rgb_to_hsv(parse_color(DEFAULT_CUSTOM_HEX).unwrap_or([255, 102, 0])).0
            } else {
                h
            }
        }
        Err(_) => rgb_to_hsv(parse_color(DEFAULT_CUSTOM_HEX).unwrap_or([255, 102, 0])).0,
    }
}

fn open_color_dial(state: &mut UiState) {
    if !uses_accent_color(&state.st.effect) {
        return;
    }
    let hue = dial_start_hue(state);
    state.color_dial = Some(ColorDial {
        hue,
        prev_color: state.st.color.clone(),
        prev_custom_hex: state.custom_hex.clone(),
    });
    apply_dial_color(state);
}

fn nudge_color_dial(state: &mut UiState, dir: i32) {
    let Some(dial) = state.color_dial.as_mut() else {
        return;
    };
    dial.hue = (dial.hue + HUE_STEP * dir as f32).rem_euclid(1.0);
    apply_dial_color(state);
}

fn apply_dial_color(state: &mut UiState) {
    let Some(hue) = state.color_dial.as_ref().map(|d| d.hue) else {
        return;
    };
    // full sat/value = best LED punch
    let hex = rgb_to_hex(hsv_to_rgb(hue, 1.0, 1.0));
    state.custom_hex = hex.clone();
    state.color_idx = CUSTOM_COLOR_IDX;
    patch_color(state, &hex);
}

fn close_color_dial(state: &mut UiState, commit: bool) {
    let Some(dial) = state.color_dial.take() else {
        return;
    };
    if commit {
        state.color_idx = CUSTOM_COLOR_IDX;
        let hex = state.custom_hex.clone();
        patch_color(state, &hex);
        return;
    }
    state.custom_hex = dial.prev_custom_hex;
    patch_color(state, &dial.prev_color);
}

// ── drawing ──────────────────────────────────────────────────────────────────

const DIM: Style = Style::new().fg(Color::DarkGray);
/// Control panel heights (with borders): 12 rows packed, or 17 under group headers.
const COMPACT_CONTROLS: u16 = 14;
const SPACED_CONTROLS: u16 = 19;
const INFO_ROWS: u16 = 7;

fn draw_ui(f: &mut Frame, state: &UiState) {
    let area = f.area();
    if area.width < 64 || area.height < 16 {
        f.render_widget(Paragraph::new("hyper-sync: make the terminal at least 64×16 (80×24 shows the strip)"), area);
        return;
    }
    let preview_rows = if area.height >= 24 { 9 } else { 0 };
    let [preview, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(preview_rows), Constraint::Min(0), Constraint::Length(1)])
        .areas(area);
    // Wide terminals: keep the panels together under the strip instead of stretching them.
    let w = body.width.min(110);
    let body = Rect { x: body.x + (body.width - w) / 2, width: w, ..body };
    let footer = Rect { x: body.x, width: w, ..footer };
    let [left, effects] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(44), Constraint::Length(36)])
        .areas(body);
    // Tall terminals get stats & config under the controls (grouped when there's room for that
    // too); otherwise `i` swaps them in.
    let split = if left.height >= SPACED_CONTROLS + INFO_ROWS {
        [Constraint::Length(SPACED_CONTROLS), Constraint::Min(0)]
    } else if left.height >= COMPACT_CONTROLS + INFO_ROWS {
        [Constraint::Length(COMPACT_CONTROLS), Constraint::Min(0)]
    } else {
        [Constraint::Min(0), Constraint::Length(0)]
    };
    let [controls, info] = Layout::default().direction(Direction::Vertical).constraints(split).areas(left);

    if preview_rows > 0 {
        draw_preview(f, preview, state);
    }
    if info.height > 0 {
        draw_controls(f, controls, state);
        draw_info(f, info, state);
    } else if state.info {
        draw_info(f, controls, state);
    } else {
        draw_controls(f, controls, state);
    }
    draw_effects(f, effects, state);
    draw_footer(f, footer, state);
    if state.help {
        draw_help(f);
    }
    if state.color_dial.is_some() {
        draw_color_dial(f, state);
    }
}

/// The strip as it sits on the monitor (17 right / 31 top / 17 left), drawn with half blocks so
/// each terminal row holds two LEDs, with every 5th LED numbered just inside it. In screen sync the
/// part of the screen being sampled is shaded with the colour taken from it.
fn draw_preview(f: &mut Frame, area: Rect, state: &UiState) {
    let st = &state.st;
    let leds: Vec<[u8; 3]> = (0..st.leds.len() / 6)
        .map(|i| std::array::from_fn(|k| u8::from_str_radix(&st.leds[i * 6 + k * 2..i * 6 + k * 2 + 2], 16).unwrap_or(0)))
        .collect();
    let n = if leds.is_empty() { 65 } else { leds.len() };
    let lit = state.connected && st.effect != "off" && leds.len() == n;
    let rgb = |i: usize| if lit { leds[i] } else { [0, 0, 0] };
    let [(_, r1), (t0, t1), (l0, l1)] = segment_bounds(n);
    let (right, top, left) = (r1, t1 - t0, l1 - l0);
    // LEDs are linear light; the terminal expects sRGB. `k` dims (for the sampled-area shading).
    let to_term = |[r, g, b]: [u8; 3], k: f32| {
        let enc = |v: u8| ((f32::from(v) / 255.0).powf(1.0 / 2.2) * 255.0 * k) as u8;
        Color::Rgb(enc(r), enc(g), enc(b))
    };
    let led = |i: usize| if rgb(i) == [0, 0, 0] { Color::Rgb(40, 40, 40) } else { to_term(rgb(i), 1.0) };
    // Pixel grid: row 0 is the top run (right to left), rows 1.. are the sides (top to bottom).
    let pixel = |c: usize, r: usize| -> Option<Color> {
        if r == 0 {
            (1..=top).contains(&c).then(|| led(t1 - c))
        } else if c == top + 1 {
            (r <= right).then(|| led(r1 - r))
        } else if c == 0 {
            (r <= left).then(|| led(l0 + r - 1))
        } else {
            None
        }
    };
    let cw = (area.width as usize / (top + 2)).clamp(1, 3);
    let width = ((top + 2) * cw) as u16;
    let x0 = area.x + area.width.saturating_sub(width) / 2;
    let rows = ((right.max(left) + 2) / 2).min(area.height as usize);
    let cell = |c: usize, y: usize, cols: usize| Rect { x: x0 + (c * cw) as u16, y: area.y + y as u16, width: (cols * cw) as u16, height: 1 };
    for y in 0..rows {
        let spans: Vec<Span> = (0..top + 2)
            .map(|c| {
                let fill = |s: &'static str, st: Style| Span::styled(s.repeat(cw), st);
                match (pixel(c, 2 * y), pixel(c, 2 * y + 1)) {
                    (Some(a), Some(b)) => fill("▀", Style::new().fg(a).bg(b)),
                    (Some(a), None) => fill("▀", Style::new().fg(a)),
                    (None, Some(b)) => fill("▄", Style::new().fg(b)),
                    (None, None) => fill(" ", Style::new()),
                }
            })
            .collect();
        f.render_widget(Paragraph::new(Line::from(spans)), cell(0, y, top + 2));
    }

    // Edge sync: the band just inside the strip stands for the screen edge each LED samples.
    let edges = lit && st.effect == "screen";
    let band = |i: usize| -> Style {
        if !edges {
            return DIM;
        }
        let [r, g, b] = rgb(i);
        let bright = 0.3 * f32::from(r) + 0.6 * f32::from(g) + 0.1 * f32::from(b) > 110.0;
        Style::new().bg(to_term(rgb(i), 0.55)).fg(if bright { Color::Black } else { Color::White })
    };
    // (column, line) of the band cell next to LED `i`; None on the corner line.
    let band_cell = |i: usize| -> Option<(usize, usize)> {
        let (c, r) = if i < r1 {
            (top, r1 - i)
        } else if i < t1 {
            return Some((t1 - i, 1));
        } else {
            (1, i - l0 + 1)
        };
        (r >= 2).then_some((c, r / 2))
    };
    if edges {
        for i in 0..n {
            if let Some((c, y)) = band_cell(i).filter(|&(_, y)| y < rows) {
                f.render_widget(Paragraph::new(" ".repeat(cw)).style(band(i)), cell(c, y, 1));
            }
        }
    }
    for i in (0..n).step_by(5).chain([n - 1]) {
        if let Some((c, y)) = band_cell(i).filter(|&(_, y)| y < rows) {
            f.render_widget(Paragraph::new(i.to_string()).style(band(i)), Rect { width: 2, ..cell(c, y, 1) });
        }
    }

    // Status inside the monitor outline; center sync shows the box it averages.
    let text = |f: &mut Frame, y: usize, line: Line<'static>| {
        if y + 1 < rows {
            f.render_widget(Paragraph::new(line).alignment(Alignment::Center), cell(2, y, top - 2));
        }
    };
    let label = mode_index(&st.effect).map_or(st.effect.as_str(), |i| MODES[i].1);
    text(f, 2, Line::styled(label.to_uppercase(), Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    if !state.connected {
        text(f, 3, Line::styled("daemon not responding — retrying", Style::new().fg(Color::Red)));
        return;
    }
    text(f, 3, Line::from(vec![
        Span::raw(format!("{:.0}/{} fps · ", st.actual_fps, st.fps)),
        if st.serial_ok {
            Span::styled("strip connected", Style::new().fg(Color::Green))
        } else {
            Span::styled("strip disconnected", Style::new().fg(Color::Red))
        },
    ]));
    let mut info_line = 4;
    if st.effect == "screen_center" && rows >= 9 {
        let margin = (top as f32 * 0.275).round() as usize;
        let shade = if lit { to_term(rgb(0), 0.6) } else { Color::Rgb(40, 40, 40) };
        for y in 4..=6 {
            let caption = if y == 5 { "sampled area" } else { "" };
            f.render_widget(
                Paragraph::new(caption).alignment(Alignment::Center).style(Style::new().bg(shade).fg(Color::White)),
                cell(1 + margin, y, top - 2 * margin),
            );
        }
        info_line = 7;
    }
    if is_screen_sync(&st.effect) && st.width > 0 {
        let what = if st.effect == "screen" {
            let band = ((st.width.min(st.height) as f32) * 0.03).clamp(16.0, 96.0);
            format!("{band:.0} px band along each edge")
        } else {
            "middle 45% of the screen".into()
        };
        text(f, info_line, Line::styled(format!("capturing {}×{} · {what}", st.width, st.height), DIM));
        info_line += 1;
    }
    if let Some(err) = &st.last_error {
        text(f, info_line, Line::styled(err.clone(), Style::new().fg(Color::Red)));
    }
}

fn bar(ratio: f32, width: usize, color: Color, enabled: bool) -> Span<'static> {
    let full = (ratio.clamp(0.0, 1.0) * width as f32).round() as usize;
    let text = format!("{}{}", "█".repeat(full), "░".repeat(width - full));
    Span::styled(text, if enabled { Style::new().fg(color) } else { DIM })
}

/// One control row: label, value spans, key hint.
fn row(label: &str, mut value: Vec<Span<'static>>, keys: &str, enabled: bool) -> Line<'static> {
    let label_style = if enabled { Style::new() } else { DIM };
    let mut spans = vec![Span::styled(format!("{label:<14}"), label_style)];
    spans.append(&mut value);
    spans.push(Span::styled(format!(" {keys}"), DIM));
    Line::from(spans)
}

fn draw_controls(f: &mut Frame, area: Rect, state: &UiState) {
    let st = &state.st;
    let on = state.connected;
    // Bars take whatever the panel has left after label, value and key hint.
    let bw = (area.width as usize).saturating_sub(2 + 14 + 6 + 5).clamp(8, 40);
    let screen = is_screen_sync(&st.effect) && on;
    let speed = uses_speed(&st.effect) && on;
    let power = uses_power(&st.effect) && on;
    let accent = uses_accent_color(&st.effect) && on;
    let sound = sound_on(st) && on;
    let audio = (sound || st.effect == "sound_viz") && on;
    let slider = |label: &str, ratio: f32, color: Color, value: String, keys: &str, enabled: bool| {
        let value = Span::styled(format!(" {value:>5}"), if enabled { Style::new() } else { DIM });
        row(label, vec![bar(ratio, bw, color, enabled), value], keys, enabled)
    };
    let pct = |v: f32| format!("{:.0}%", v * 100.0);
    let picker = |items: &[String], selected: &str, color: Color| -> Vec<Span<'static>> {
        items
            .iter()
            .map(|it| {
                if it == selected {
                    Span::styled(format!("[{it}]"), Style::new().fg(color).add_modifier(Modifier::BOLD))
                } else {
                    Span::raw(format!(" {it} "))
                }
            })
            .collect()
    };

    // Color presets as swatches; the selected one is a diamond. Rainbow cycles its hue.
    let mut swatches: Vec<Span> = (0..COLOR_SLOT_COUNT)
        .map(|i| {
            let hex = if i == CUSTOM_COLOR_IDX { state.custom_hex.as_str() } else { COLOR_PRESETS[i] };
            let rgb = if is_rainbow_color(hex) {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                hsv_to_rgb((t.as_secs_f32() * 0.25).fract(), 1.0, 1.0)
            } else {
                parse_color(hex).unwrap_or([128, 128, 128])
            };
            let glyph = if i == state.color_idx { "◆ " } else { "■ " };
            Span::styled(glyph, if accent { Style::new().fg(Color::Rgb(rgb[0], rgb[1], rgb[2])) } else { DIM })
        })
        .collect();
    // The hex only when the row has room for it (not at 80 columns).
    if area.width as usize >= 2 + 14 + 2 * COLOR_SLOT_COUNT + 8 + 5 {
        swatches.push(Span::styled(
            if is_rainbow_color(&st.color) { "rainbow".to_string() } else { st.color.clone() },
            if accent { Style::new() } else { DIM },
        ));
    }

    let tune = if st.effect == "candle" {
        slider("warmth", st.warmth, Color::LightRed, pct(st.warmth), "[ ]", on)
    } else if is_screen_sync(&st.effect) {
        slider("gamma", (st.gamma - 1.0) / 2.0, Color::LightBlue, format!("{:.1}", st.gamma), "[ ]", on)
    } else {
        row("warmth", vec![Span::styled("candle (gamma: screen)", DIM)], "", false)
    };
    let screen_tau = 0.6 * st.screen_smoothing.powi(2) * 1000.0;
    let fps: Vec<String> = FPS_PRESETS.iter().map(u32::to_string).collect();
    let sound_modes: Vec<String> = SOUND_MODES.iter().map(|m| m.to_string()).collect();

    // A tiny spectrum while audio is running (sound modes or Sound Viz).
    const TICKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let spectrum = if st.spectrum.is_empty() {
        row("audio", vec![Span::styled("off", DIM)], "", false)
    } else {
        let mut spans: Vec<Span> = st
            .spectrum
            .iter()
            .map(|&v| Span::styled(TICKS[((v.clamp(0.0, 1.0) * 7.0).round()) as usize], Style::new().fg(Color::Magenta)))
            .collect();
        spans.push(Span::raw(format!("  {:.0}%", st.audio_level * 100.0)));
        row("audio", spans, "", true)
    };

    let screen_smooth = if screen_tau < 1.0 { "off".into() } else { format!("{screen_tau:.0}ms") };
    let sound_smooth = format!("{:.0}ms", 50.0 + 550.0 * st.audio_smoothing);
    let lines = if area.height >= SPACED_CONTROLS {
        // Room to spare: group the rows under what they act on.
        let header = |name: &str| Line::styled(format!("─ {name}"), DIM.add_modifier(Modifier::BOLD));
        let candle = st.effect == "candle" && on;
        vec![
            header("light"),
            slider("brightness", st.brightness, Color::Cyan, pct(st.brightness), "w/s", on),
            row("fps", picker(&fps, &st.fps.to_string(), Color::Cyan), "-/=", on),
            header("effect"),
            slider("speed", (st.speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN), Color::Yellow, format!("{:.1}×", st.speed), "a/d", speed),
            slider("power", st.power, Color::LightRed, pct(st.power), "z/x", power),
            row("color", swatches, "←→ c", accent),
            slider("warmth", st.warmth, Color::LightRed, pct(st.warmth), "[ ]", candle),
            header("screen sync"),
            slider("gamma", (st.gamma - 1.0) / 2.0, Color::LightBlue, format!("{:.1}", st.gamma), "[ ]", screen),
            slider("smoothing", st.screen_smoothing, Color::LightBlue, screen_smooth, "{ }", screen),
            header("sound"),
            row("mode", picker(&sound_modes, &st.sound_mode, Color::Magenta), "Tab", on),
            slider("reaction", st.reactivity, Color::Green, pct(st.reactivity), "j/k", sound),
            slider("sensitivity", st.sensitivity, Color::LightBlue, pct(st.sensitivity), "h/l", sound),
            slider("smoothing", st.audio_smoothing, Color::Magenta, sound_smooth, ", .", audio),
            spectrum,
        ]
    } else {
        vec![
            slider("brightness", st.brightness, Color::Cyan, pct(st.brightness), "w/s", on),
            slider("speed", (st.speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN), Color::Yellow, format!("{:.1}×", st.speed), "a/d", speed),
            slider("power", st.power, Color::LightRed, pct(st.power), "z/x", power),
            row("fps", picker(&fps, &st.fps.to_string(), Color::Cyan), "-/=", on),
            row("color", swatches, "←→ c", accent),
            tune,
            slider("screen smooth", st.screen_smoothing, Color::LightBlue, screen_smooth, "{ }", screen),
            row("sound", picker(&sound_modes, &st.sound_mode, Color::Magenta), "Tab", on),
            slider("reaction", st.reactivity, Color::Green, pct(st.reactivity), "j/k", sound),
            slider("sensitivity", st.sensitivity, Color::LightBlue, pct(st.sensitivity), "h/l", sound),
            slider("sound smooth", st.audio_smoothing, Color::Magenta, sound_smooth, ", .", audio),
            spectrum,
        ]
    };
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" controls ")), area);
}

/// Favorites on top, then every effect under its group header; scrolls to keep the cursor in view.
fn draw_effects(f: &mut Frame, area: Rect, state: &UiState) {
    let title = if state.digits.is_empty() { " effects ".to_string() } else { format!(" effects → {}_ ", state.digits) };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let entry = |pos: usize, idx: usize| -> Line<'static> {
        let (mode, label, _, hyperion) = MODES[idx];
        let (active, cursor) = (mode == state.st.effect, pos == state.cursor);
        let style = if active {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if cursor {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new()
        };
        let mut spans = vec![
            Span::styled(if cursor { "▶" } else { " " }, Style::new().fg(Color::Cyan)),
            Span::styled(format!("{idx:>2} "), DIM),
            Span::styled(label, style),
        ];
        if hyperion {
            spans.push(Span::styled(" H", DIM));
        }
        if pos > state.favorites.len() && state.favorites.contains(&idx) {
            spans.push(Span::styled(" ★", Style::new().fg(Color::Yellow)));
        }
        Line::from(spans)
    };
    // Off and the favorites stay pinned on top (up to half the panel); the rest scrolls below.
    let mut favs = vec![Line::styled("★ favorites", Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))];
    let mut fav_cursor = None;
    for (pos, idx) in std::iter::once(0).chain(state.favorites.iter().copied()).enumerate() {
        if pos == state.cursor {
            fav_cursor = Some(favs.len());
        }
        favs.push(entry(pos, idx));
    }
    let mut all = Vec::new();
    let mut all_cursor = None;
    let mut group = "";
    for (idx, m) in MODES.iter().enumerate().skip(1) {
        if m.2 != group {
            group = m.2;
            all.push(Line::styled(format!("─ {group}"), DIM.add_modifier(Modifier::BOLD)));
        }
        let pos = state.favorites.len() + idx;
        if pos == state.cursor {
            all_cursor = Some(all.len());
        }
        all.push(entry(pos, idx));
    }
    let fav_rows = favs.len().min(inner.height as usize / 2) as u16;
    let [fav_area, all_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(fav_rows), Constraint::Min(0)])
        .areas(inner);
    // Keep the cursor (and the line above it) in view without jumping on every move.
    let mut show = |lines: Vec<Line<'static>>, cursor: Option<usize>, area: Rect, scroll: &std::cell::Cell<usize>| {
        let rows = area.height as usize;
        let mut top = scroll.get();
        if let Some(line) = cursor {
            if line < top + 1 {
                top = line.saturating_sub(1);
            }
            if line + 1 > top + rows {
                top = line + 1 - rows;
            }
        }
        top = top.min(lines.len().saturating_sub(rows));
        scroll.set(top);
        f.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), area);
    };
    show(favs, fav_cursor, fav_area, &state.scroll[0]);
    show(all, all_cursor, all_area, &state.scroll[1]);
}

/// Stats from the daemon and the config file at a glance.
fn draw_info(f: &mut Frame, area: Rect, state: &UiState) {
    let st = &state.st;
    let line = |label: &str, mut value: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{label:<8}"), DIM)];
        spans.append(&mut value);
        Line::from(spans)
    };
    let amp_style = |a: f32| Style::new().fg(if a < 0.5 { Color::Green } else if a < 0.9 { Color::Yellow } else { Color::Red });
    let up = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(st.started);
    let uptime = if st.started == 0 { "?".into() } else if up >= 3600 { format!("{}h {}m", up / 3600, up / 60 % 60) } else { format!("{}m {}s", up / 60, up % 60) };
    let home = std::env::var("HOME").unwrap_or_default();
    let path = crate::config::runtime_config_path().display().to_string();
    let path = if !home.is_empty() && path.starts_with(&home) { format!("~{}", &path[home.len()..]) } else { path };
    let mut lines = vec![
        line("daemon", vec![Span::raw(format!("{:.0}/{} fps · up {uptime}", st.actual_fps, st.fps))]),
        line("current", vec![
            Span::styled(format!("≈{:.2} A", state.amps), amp_style(state.amps)),
            Span::raw(" now · "),
            Span::styled(format!("{:.2} A", state.peak_amps), amp_style(state.peak_amps)),
            Span::styled(" peak (est.)", DIM),
        ]),
    ];
    if let Some(cfg) = &state.cfg {
        let [(_, r1), (t0, t1), (l0, l1)] = segment_bounds(usize::from(cfg.device.leds));
        lines.push(line("strip", vec![Span::raw(format!("{} LEDs ({}/{}/{}) · {}", cfg.device.leds, r1, t1 - t0, l1 - l0, cfg.device.port))]));
        #[cfg(feature = "screen")]
        lines.push(line("screen", vec![Span::raw(format!("monitor {} · {}", cfg.screen.monitor, cfg.screen.layout))]));
    }
    lines.push(line("config", vec![Span::raw(path), Span::styled("  e edit", DIM)]));
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" stats & config ")), area);
}

fn draw_footer(f: &mut Frame, area: Rect, state: &UiState) {
    let hint = if state.color_dial.is_some() {
        "←/→ hue · Enter keep · Esc cancel"
    } else if state.browse_since.is_some() {
        "↑/↓ browse · Enter start now · starts when you pause"
    } else {
        "? help · ↑/↓ or number: effect · f favorite · i info · q quit"
    };
    f.render_widget(Paragraph::new(Span::styled(hint, DIM)), area);
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area.width.saturating_mul(percent_x) / 100;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width, height: height.min(area.height) }
}

fn draw_help(f: &mut Frame) {
    const KEYS: &[(&str, &str)] = &[
        ("↑/↓, Enter", "browse; starts when you pause (Enter: now)"),
        ("0–45", "jump to an effect by number (0 = off)"),
        ("f", "favorite: start it and pin it to the top (again: unpin)"),
        ("w/s", "brightness"),
        ("a/d", "speed (animated effects)"),
        ("z/x", "power: how strongly effects dim, flicker, contrast"),
        ("- / =", "frame rate"),
        ("←/→, c", "color preset · custom color dial"),
        ("[ / ]", "candle warmth · screen gamma"),
        ("{ / }", "screen smoothing (calmer, but lags)"),
        ("Tab", "sound mode: off · level · balance"),
        ("j/k", "sound reaction: dims on quiet, lifts on beats"),
        ("h/l", "sound sensitivity"),
        (", / .", "sound smoothing: how slowly it falls back"),
        ("i", "stats & config (always shown on tall terminals)"),
        ("e", "edit config.toml in $EDITOR (applies live)"),
        ("R", "pick another screen to capture"),
        ("Ctrl+R", "restart the current effect"),
        ("q, Esc", "quit (the daemon keeps running)"),
    ];
    let area = centered_rect(90, KEYS.len() as u16 + 2, f.area());
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| Line::from(vec![Span::styled(format!("{k:>11}  "), Style::new().fg(Color::Cyan)), Span::raw(*what)]))
        .collect();
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" keys — any key closes ").border_style(Style::new().fg(Color::Cyan))),
        area,
    );
}

/// Hue dial: a marker above the full hue bar (it wraps around: left of red is magenta), and the
/// chosen color shown on its own line below so it can't be mistaken for part of the bar.
fn draw_color_dial(f: &mut Frame, state: &UiState) {
    let Some(dial) = state.color_dial.as_ref() else {
        return;
    };
    let area = centered_rect(70, 7, f.area());
    let block = Block::default().borders(Borders::ALL).title(" custom color ").border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if inner.width < 10 || inner.height < 4 {
        return;
    }
    let line = |y: u16| Rect { y: inner.y + y, height: 1, ..inner };
    let w = inner.width;
    let marker = ((dial.hue * w as f32) as u16).min(w - 1);
    f.render_widget(Paragraph::new(format!("{}▼", " ".repeat(marker as usize))).style(Style::new().add_modifier(Modifier::BOLD)), line(0));
    let bar: Vec<Span> = (0..w)
        .map(|i| {
            let [r, g, b] = hsv_to_rgb(i as f32 / w as f32, 1.0, 1.0);
            Span::styled("█", Style::new().fg(Color::Rgb(r, g, b)))
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(bar)), line(1));
    let [r, g, b] = hsv_to_rgb(dial.hue, 1.0, 1.0);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("selected "),
            Span::styled("      ", Style::new().bg(Color::Rgb(r, g, b))),
            Span::raw(format!(" #{}", rgb_to_hex([r, g, b]))),
        ])),
        line(3),
    );
    if inner.height >= 5 {
        f.render_widget(Paragraph::new(Span::styled("←→ hue (wraps around) · Enter keep · Esc cancel", DIM)), line(4));
    }
}
