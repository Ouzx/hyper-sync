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

/// Effects in display order (screen, static, ambient, motion) with their labels.
const MODES: &[(&str, &str)] = &[
    ("off", "Off"),
    ("screen", "Screen Sync"),
    ("screen_center", "Screen Center"),
    ("sound_viz", "Sound Viz"),
    ("solid", "Solid"),
    ("flow", "Rainbow Flow"),
    ("candle", "Candle"),
    ("fire", "Fire"),
    ("lava", "Lava"),
    ("aurora", "Aurora"),
    ("pulse", "Pulse"),
    ("heartbeat", "Heartbeat"),
    ("sunrise", "Sunrise"),
    ("chase", "Chase"),
    ("wave", "Wave"),
    ("scanner", "Scanner"),
    ("sparkle", "Sparkle"),
    ("lightning", "Lightning"),
    ("segment", "Segment"),
    ("strobe", "Strobe"),
    ("wipe", "Wipe"),
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
    cursor: usize,
    browse_since: Option<Instant>,
    digits: String,
    digits_since: Instant,
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
        cursor: 0,
        browse_since: None,
        digits: String::new(),
        digits_since: Instant::now(),
    };
    poll_status(&mut state);
    state.cursor = mode_index(&state.st.effect).unwrap_or(0);
    let mut last_poll = Instant::now();

    while !quit.load(Ordering::Relaxed) {
        terminal.draw(|f| draw_ui(f, &state))?;

        // Sleep until a key arrives or the next poll is due — no busy redraw.
        if event::poll(POLL.saturating_sub(last_poll.elapsed()))? {
            while event::poll(Duration::ZERO)? {
                if let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? {
                    if kind != KeyEventKind::Release && handle_key(&mut state, code, modifiers) {
                        return Ok(());
                    }
                }
            }
        }
        if state.browse_since.is_some_and(|t| t.elapsed() >= BROWSE_SETTLE) {
            state.browse_since = None;
            let idx = state.cursor;
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
            state.cursor = (state.cursor as i32 + dir).rem_euclid(MODES.len() as i32) as usize;
            state.browse_since = Some(Instant::now());
        }
        KeyCode::Enter => {
            let idx = state.cursor;
            select_mode(state, idx);
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            state.digits.push(c);
            state.digits_since = Instant::now();
            // Apply as soon as no longer number could start with what was typed.
            if state.digits.parse::<usize>().unwrap_or(0) * 10 > MODES.len() {
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
        state.cursor = mode_index(&st.effect).unwrap_or(state.cursor);
    }
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
    state.cursor = idx;
    state.browse_since = None;
    if MODES[idx].0 != state.st.effect {
        send(state, Patch { mode: Some(MODES[idx].0.into()), ..Default::default() });
    }
}

fn apply_digits(state: &mut UiState) {
    let pick = state.digits.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
    state.digits.clear();
    if let Some(idx) = pick.filter(|&i| i < MODES.len()) {
        select_mode(state, idx);
    }
}

fn mode_index(mode: &str) -> Option<usize> {
    MODES.iter().position(|(m, _)| *m == mode)
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
    !matches!(effect, "off" | "solid" | "sound_viz") && !is_screen_sync(effect)
}

fn uses_accent_color(effect: &str) -> bool {
    !matches!(effect, "off" | "fire" | "flow" | "sunrise") && !is_screen_sync(effect)
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
    let [controls, effects] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(44), Constraint::Length(36)])
        .areas(body);

    if preview_rows > 0 {
        draw_preview(f, preview, state);
    }
    draw_controls(f, controls, state);
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
/// each terminal row holds two LEDs. Colors are what the LEDs get, converted for the screen.
fn draw_preview(f: &mut Frame, area: Rect, state: &UiState) {
    let st = &state.st;
    let leds: Vec<[u8; 3]> = (0..st.leds.len() / 6)
        .map(|i| std::array::from_fn(|k| u8::from_str_radix(&st.leds[i * 6 + k * 2..i * 6 + k * 2 + 2], 16).unwrap_or(0)))
        .collect();
    let n = if leds.is_empty() { 65 } else { leds.len() };
    let lit = state.connected && st.effect != "off" && leds.len() == n;
    let [(_, r1), (t0, t1), (l0, l1)] = segment_bounds(n);
    let (right, top, left) = (r1, t1 - t0, l1 - l0);
    let led = |i: usize| -> Color {
        let [r, g, b] = if lit { leds[i] } else { [0, 0, 0] };
        if r == 0 && g == 0 && b == 0 {
            return Color::Rgb(40, 40, 40); // keep the strip's outline visible when dark
        }
        // LEDs are linear light; the terminal expects sRGB.
        let enc = |v: u8| ((f32::from(v) / 255.0).powf(1.0 / 2.2) * 255.0) as u8;
        Color::Rgb(enc(r), enc(g), enc(b))
    };
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
    let rows = (right.max(left) + 2) / 2;
    for y in 0..rows.min(area.height as usize) {
        let spans: Vec<Span> = (0..top + 2)
            .map(|c| {
                let cell = |s: &'static str, st: Style| Span::styled(s.repeat(cw), st);
                match (pixel(c, 2 * y), pixel(c, 2 * y + 1)) {
                    (Some(a), Some(b)) => cell("▀", Style::new().fg(a).bg(b)),
                    (Some(a), None) => cell("▀", Style::new().fg(a)),
                    (None, Some(b)) => cell("▄", Style::new().fg(b)),
                    (None, None) => cell(" ", Style::new()),
                }
            })
            .collect();
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { x: x0, y: area.y + y as u16, width, height: 1 });
    }

    // Status inside the monitor outline.
    let inner = Rect {
        x: x0 + cw as u16 + 1,
        y: area.y + 2,
        width: (top * cw) as u16 - 2,
        height: (rows as u16).saturating_sub(3),
    };
    let label = mode_index(&st.effect).map_or(st.effect.as_str(), |i| MODES[i].1);
    let mut lines = vec![Line::from(Span::styled(label.to_uppercase(), Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)))];
    if !state.connected {
        lines.push(Line::styled("daemon not responding — retrying", Style::new().fg(Color::Red)));
    } else {
        lines.push(Line::from(vec![
            Span::raw(format!("{:.0}/{} fps · ", st.actual_fps, st.fps)),
            if st.serial_ok {
                Span::styled("strip connected", Style::new().fg(Color::Green))
            } else {
                Span::styled("strip disconnected", Style::new().fg(Color::Red))
            },
        ]));
        if is_screen_sync(&st.effect) && st.width > 0 {
            lines.push(Line::styled(format!("capturing {}×{}", st.width, st.height), DIM));
        }
        if let Some(err) = &st.last_error {
            lines.push(Line::styled(err.clone(), Style::new().fg(Color::Red)));
        }
    }
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn bar(ratio: f32, width: usize, color: Color, enabled: bool) -> Span<'static> {
    let full = (ratio.clamp(0.0, 1.0) * width as f32).round() as usize;
    let text = format!("{}{}", "█".repeat(full), "░".repeat(width - full));
    Span::styled(text, if enabled { Style::new().fg(color) } else { DIM })
}

/// One control row: label, value spans, key hint.
fn row(label: &str, mut value: Vec<Span<'static>>, keys: &str, enabled: bool) -> Line<'static> {
    let label_style = if enabled { Style::new() } else { DIM };
    let mut spans = vec![Span::styled(format!("{label:<12}"), label_style)];
    spans.append(&mut value);
    spans.push(Span::styled(format!(" {keys}"), DIM));
    Line::from(spans)
}

fn draw_controls(f: &mut Frame, area: Rect, state: &UiState) {
    let st = &state.st;
    // Bars take whatever the panel has left after label, value and key hint.
    let bw = (area.width as usize).saturating_sub(2 + 12 + 6 + 5).clamp(8, 40);
    let on = state.connected;
    let sound = sound_on(st);
    let speed = uses_speed(&st.effect);
    let accent = uses_accent_color(&st.effect);
    let val = |s: String, enabled: bool| Span::styled(format!(" {s:>5}"), if enabled { Style::new() } else { DIM });

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
            Span::styled(glyph, if accent && on { Style::new().fg(Color::Rgb(rgb[0], rgb[1], rgb[2])) } else { DIM })
        })
        .collect();
    swatches.push(Span::styled(
        if is_rainbow_color(&st.color) { "rainbow".to_string() } else { st.color.clone() },
        if accent { Style::new() } else { DIM },
    ));

    let fps_spans: Vec<Span> = FPS_PRESETS
        .iter()
        .map(|&p| {
            if p == st.fps {
                Span::styled(format!("[{p}]"), Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else {
                Span::raw(format!(" {p} "))
            }
        })
        .collect();

    let tune = if st.effect == "candle" {
        row("warmth", vec![bar(st.warmth, bw, Color::LightRed, on), val(format!("{:.0}%", st.warmth * 100.0), on)], "[/]", on)
    } else if is_screen_sync(&st.effect) {
        row("gamma", vec![bar((st.gamma - 1.0) / 2.0, bw, Color::LightBlue, on), val(format!("{:.1}", st.gamma), on)], "[/]", on)
    } else {
        row("tune", vec![Span::styled("not used by this effect", DIM)], "", false)
    };

    let sound_spans: Vec<Span> = SOUND_MODES
        .iter()
        .map(|&m| {
            if m == st.sound_mode {
                Span::styled(format!("[{m}]"), Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD))
            } else {
                Span::raw(format!(" {m} "))
            }
        })
        .collect();

    // A tiny spectrum while audio is running (sound modes or Sound Viz).
    const TICKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let audio = if st.spectrum.is_empty() {
        row("audio", vec![Span::styled("off", DIM)], "", false)
    } else {
        let mut spans: Vec<Span> = st
            .spectrum
            .iter()
            .map(|&v| Span::styled(TICKS[((v.clamp(0.0, 1.0) * 7.0).round()) as usize], Style::new().fg(Color::Magenta)))
            .collect();
        spans.push(val(format!("{:.0}%", st.audio_level * 100.0), true));
        row("audio", spans, "", true)
    };

    let lines = vec![
        row("brightness", vec![bar(st.brightness, bw, Color::Cyan, on), val(format!("{:.0}%", st.brightness * 100.0), on)], "w/s", on),
        row("speed", vec![bar((st.speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN), bw, Color::Yellow, speed && on), val(format!("{:.1}×", st.speed), speed && on)], "a/d", speed && on),
        row("fps", fps_spans, "-/=", on),
        row("color", swatches, "←→ c", accent && on),
        tune,
        Line::raw(""),
        row("sound", sound_spans, "Tab", on),
        row("boost", vec![bar(st.reactivity, bw, Color::Green, sound && on), val(format!("{:.0}%", st.reactivity * 100.0), sound && on)], "j/k", sound && on),
        row("sensitivity", vec![bar(st.sensitivity, bw, Color::LightBlue, sound && on), val(format!("{:.0}%", st.sensitivity * 100.0), sound && on)], "h/l", sound && on),
        audio,
    ];
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" controls ")), area);
}

fn draw_effects(f: &mut Frame, area: Rect, state: &UiState) {
    let title = if state.digits.is_empty() { " effects ".to_string() } else { format!(" effects → {}_ ", state.digits) };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let rows = inner.height as usize;
    let cols = MODES.len().div_ceil(rows);
    let col_w = inner.width / cols as u16;
    for (i, (mode, label)) in MODES.iter().enumerate() {
        let (col, r) = (i / rows, i % rows);
        let active = *mode == state.st.effect;
        let cursor = i == state.cursor;
        let style = if active {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if cursor {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new()
        };
        let line = Line::from(vec![
            Span::styled(if cursor { "▶" } else { " " }, Style::new().fg(Color::Cyan)),
            Span::styled(format!("{:>2} ", i + 1), DIM),
            Span::styled(*label, style),
        ]);
        let cell = Rect { x: inner.x + col as u16 * col_w, y: inner.y + r as u16, width: col_w, height: 1 };
        f.render_widget(Paragraph::new(line), cell);
    }
}

fn draw_footer(f: &mut Frame, area: Rect, state: &UiState) {
    let hint = if state.color_dial.is_some() {
        "←/→ hue · Enter keep · Esc cancel"
    } else if state.browse_since.is_some() {
        "↑/↓ browse · Enter start now · starts when you pause"
    } else {
        "? help · ↑/↓ or number: effect · q quit"
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
        ("1–21", "jump to an effect by number"),
        ("w/s", "brightness"),
        ("a/d", "speed (animated effects)"),
        ("- / =", "frame rate"),
        ("←/→", "color preset"),
        ("c", "custom color dial"),
        ("[ / ]", "candle warmth · screen gamma"),
        ("Tab", "sound mode: off · level · balance"),
        ("j/k", "audio boost"),
        ("h/l", "audio sensitivity"),
        ("R", "pick another screen to capture"),
        ("Ctrl+R", "restart the current effect"),
        ("q, Esc", "quit (the daemon keeps running)"),
    ];
    let area = centered_rect(70, KEYS.len() as u16 + 2, f.area());
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

fn draw_color_dial(f: &mut Frame, state: &UiState) {
    let Some(dial) = state.color_dial.as_ref() else {
        return;
    };
    let area = centered_rect(70, 7, f.area());
    let block = Block::default()
        .borders(Borders::ALL)
        .title("custom color")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let rgb = hsv_to_rgb(dial.hue, 1.0, 1.0);
    let hex = rgb_to_hex(rgb);

    let swatch = Rect {
        x: inner.x,
        y: inner.y,
        width: 5.min(inner.width),
        height: 1,
    };
    if swatch.width > 0 {
        f.render_widget(
            Paragraph::new("     ").style(Style::default().bg(Color::Rgb(rgb[0], rgb[1], rgb[2]))),
            swatch,
        );
    }

    let bar_x = inner.x.saturating_add(6);
    let bar_w = inner.width.saturating_sub(6);
    if bar_w > 0 {
        let marker = ((dial.hue * bar_w as f32) as u16).min(bar_w.saturating_sub(1));
        let mut spans = Vec::with_capacity(bar_w as usize);
        for i in 0..bar_w {
            let [r, g, b] = hsv_to_rgb(i as f32 / bar_w as f32, 1.0, 1.0);
            let ch = if i == marker { "▼" } else { "█" };
            spans.push(Span::styled(
                ch,
                Style::default().fg(Color::Rgb(r, g, b)).add_modifier(
                    if i == marker {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                ),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                x: bar_x,
                y: inner.y,
                width: bar_w,
                height: 1,
            },
        );
    }

    if inner.height >= 3 {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!("#{hex}  ")),
                Span::styled(
                    "←→ hue · Enter set · Esc cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: 1,
            },
        );
    }
}
