use std::io::stdout;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};
use ratatui::Frame;

use crate::daemon::{daemon_running, ipc_request, IpcRequest};
use crate::effects::solid::{is_rainbow_color, parse_color, rainbow_pixel};

const MODES: &[&str] = &[
    "off",
    "screen",
    "screen_center",
    "solid",
    "candle",
    "chase",
    "wave",
    "scanner",
    "sparkle",
    "pulse",
    "aurora",
    "fire",
    "heartbeat",
    "segment",
    "strobe",
    "wipe",
    "sound_viz",
];
const SOUND_MODES: &[&str] = &["off", "level", "balance"];
const COLOR_PRESETS: &[&str] = &[
    "ff3300", "ff0000", "ffffff", "0099ff", "00ff88", "ff00ff", "ffff00", "rainbow",
];
const FPS_PRESETS: &[u32] = &[24, 30, 45, 60];
const SPEED_MIN: f32 = 0.1;
const SPEED_MAX: f32 = 5.0;

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

struct UiState {
    effect: String,
    brightness: f32,
    fps: u32,
    speed: f32,
    detail: String,
    color: String,
    width: u32,
    height: u32,
    serial_ok: bool,
    last_error: Option<String>,
    sound_mode: String,
    audio_level: f32,
    reactivity: f32,
    sensitivity: f32,
    color_idx: usize,
    fps_idx: usize,
    list_cursor: usize,
    digit_buf: String,
    list_scroll: usize,
}

impl UiState {
    fn sync_color_idx(&mut self) {
        if let Some(i) = COLOR_PRESETS.iter().position(|c| *c == self.color.as_str()) {
            self.color_idx = i;
        }
    }
    fn sync_fps_idx(&mut self) {
        if let Some(i) = FPS_PRESETS.iter().position(|&f| f == self.fps) {
            self.fps_idx = i;
            return;
        }
        self.fps_idx = FPS_PRESETS
            .iter()
            .enumerate()
            .min_by_key(|(_, &f)| f.abs_diff(self.fps))
            .map(|(i, _)| i)
            .unwrap_or(1);
    }
    fn sync_list_cursor(&mut self) {
        if let Some(i) = MODES.iter().position(|m| *m == self.effect.as_str()) {
            self.list_cursor = i;
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            effect: "off".into(),
            brightness: 0.15,
            fps: 30,
            speed: 1.0,
            detail: String::new(),
            color: "rainbow".into(),
            width: 0,
            height: 0,
            serial_ok: true,
            last_error: None,
            sound_mode: "off".into(),
            audio_level: 0.0,
            reactivity: 0.3,
            sensitivity: 0.3,
            color_idx: 7,
            fps_idx: 1,
            list_cursor: 0,
            digit_buf: String::new(),
            list_scroll: 0,
        }
    }
}

fn tui_loop(quit: Arc<AtomicBool>) -> anyhow::Result<()> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?;
    let mut state = UiState::default();
    let mut last_poll = Instant::now() - Duration::from_secs(1);

    loop {
        if quit.load(Ordering::Relaxed) {
            return Ok(());
        }

        while event::poll(Duration::from_millis(0))? {
            if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
                match code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if !state.digit_buf.is_empty() {
                            state.digit_buf.clear();
                        } else {
                            return Ok(());
                        }
                    }
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Char('w') | KeyCode::Char('W') => {
                        adjust_brightness(0.05).ok();
                    }
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        adjust_brightness(-0.05).ok();
                    }
                    KeyCode::Up => move_list_cursor(&mut state, -1),
                    KeyCode::Down => move_list_cursor(&mut state, 1),
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        if state.digit_buf.is_empty() {
                            if let Some(d) = c.to_digit(10) {
                                let n = d as usize;
                                if (1..=9).contains(&n) && n <= MODES.len() {
                                    state.list_cursor = n - 1;
                                    patch_mode(MODES[n - 1], &mut state).ok();
                                    continue;
                                }
                            }
                        }
                        state.digit_buf.push(c);
                        if state.digit_buf.len() >= 2 {
                            apply_digit_pick(&mut state);
                        }
                    }
                    KeyCode::Left => cycle_color(&mut state, -1),
                    KeyCode::Right => cycle_color(&mut state, 1),
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        adjust_speed(-0.1).ok();
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        adjust_speed(0.1).ok();
                    }
                    KeyCode::Tab => cycle_sound_mode(&mut state),
                    KeyCode::Char('j') | KeyCode::Char('J') => {
                        adjust_reactivity(-0.05).ok();
                    }
                    KeyCode::Char('k') | KeyCode::Char('K') => {
                        adjust_reactivity(0.05).ok();
                    }
                    KeyCode::Char('h') | KeyCode::Char('H') => {
                        adjust_sensitivity(-0.05).ok();
                    }
                    KeyCode::Char('l') | KeyCode::Char('L') => {
                        adjust_sensitivity(0.05).ok();
                    }
                    KeyCode::Char('-') => cycle_fps(&mut state, -1),
                    KeyCode::Char('=') | KeyCode::Char('+') => cycle_fps(&mut state, 1),
                    KeyCode::Char('R') => {
                        ipc_request(&IpcRequest::ReselectScreen).ok();
                    }
                    KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                        ipc_request(&IpcRequest::Restart).ok();
                    }
                    _ => {}
                }
            }
        }

        if last_poll.elapsed() >= Duration::from_millis(200) {
            if let Ok(resp) = ipc_request(&IpcRequest::Status) {
                if let Some(st) = resp.status {
                    state.effect = st.effect;
                    state.brightness = st.brightness;
                    state.fps = st.fps;
                    state.speed = st.speed;
                    state.detail = st.detail;
                    state.color = st.color;
                    state.width = st.width;
                    state.height = st.height;
                    state.serial_ok = st.serial_ok;
                    state.last_error = st.last_error;
                    state.sound_mode = st.sound_mode;
                    state.audio_level = st.audio_level;
                    state.reactivity = st.reactivity;
                    state.sensitivity = st.sensitivity;
                    state.sync_color_idx();
                    state.sync_fps_idx();
                    state.sync_list_cursor();
                }
            }
            last_poll = Instant::now();
        }

        terminal.draw(|f| draw_ui(f, &mut state))?;
        thread::sleep(Duration::from_millis(16));
    }
}

fn move_list_cursor(state: &mut UiState, dir: i32) {
    state.digit_buf.clear();
    let n = MODES.len() as i32;
    let next = (state.list_cursor as i32 + dir).rem_euclid(n) as usize;
    if next == state.list_cursor {
        return;
    }
    state.list_cursor = next;
    patch_mode(MODES[next], state).ok();
}

fn item_height(idx: usize, active_effect: &str) -> u16 {
    if MODES[idx] == active_effect {
        3
    } else {
        1
    }
}

fn ensure_list_cursor_visible(state: &mut UiState, visible: u16) {
    if state.list_cursor < state.list_scroll {
        state.list_scroll = state.list_cursor;
    }
    loop {
        let mut y = 0u16;
        let mut idx = state.list_scroll;
        let mut ok = false;
        while idx < MODES.len() && y < visible {
            let h = item_height(idx, &state.effect);
            if idx == state.list_cursor {
                ok = y.saturating_add(h) <= visible;
                break;
            }
            y = y.saturating_add(h);
            idx += 1;
        }
        if ok || state.list_scroll >= state.list_cursor {
            break;
        }
        state.list_scroll += 1;
    }
}

fn apply_digit_pick(state: &mut UiState) {
    let idx = state
        .digit_buf
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|&i| i < MODES.len())
        .unwrap_or(state.list_cursor);
    state.digit_buf.clear();
    state.list_cursor = idx;
    patch_mode(MODES[idx], state).ok();
}

fn cycle_color(state: &mut UiState, dir: i32) {
    if !uses_accent_color(&state.effect) {
        return;
    }
    let n = COLOR_PRESETS.len() as i32;
    state.color_idx = (state.color_idx as i32 + dir).rem_euclid(n) as usize;
    state.color = COLOR_PRESETS[state.color_idx].into();
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: None,
        color: Some(state.color.clone()),
        fps: None,
        speed: None,
        sound_mode: None,
        reactivity: None,
        sensitivity: None,
    })
    .ok();
}

fn cycle_fps(state: &mut UiState, dir: i32) {
    let n = FPS_PRESETS.len() as i32;
    state.fps_idx = (state.fps_idx as i32 + dir).rem_euclid(n) as usize;
    state.fps = FPS_PRESETS[state.fps_idx];
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: None,
        color: None,
        fps: Some(state.fps),
        speed: None,
        sound_mode: None,
        reactivity: None,
        sensitivity: None,
    })
    .ok();
}

fn cycle_sound_mode(state: &mut UiState) {
    let next = (SOUND_MODES
        .iter()
        .position(|m| *m == state.sound_mode.as_str())
        .unwrap_or(0)
        + 1)
        % SOUND_MODES.len();
    state.sound_mode = SOUND_MODES[next].to_string();
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: None,
        color: None,
        fps: None,
        speed: None,
        sound_mode: Some(state.sound_mode.clone()),
        reactivity: None,
        sensitivity: None,
    })
    .ok();
}

fn patch_mode(mode: &str, state: &mut UiState) -> anyhow::Result<()> {
    state.effect = mode.to_string();
    ipc_request(&IpcRequest::Patch {
        mode: Some(mode.into()),
        brightness: None,
        color: None,
        fps: None,
        speed: None,
        sound_mode: None,
        reactivity: None,
        sensitivity: None,
    })?;
    Ok(())
}

fn adjust_brightness(delta: f32) -> anyhow::Result<()> {
    let resp = ipc_request(&IpcRequest::Status)?;
    let b = resp
        .status
        .map(|s| (s.brightness + delta).clamp(0.0, 1.0))
        .unwrap_or(0.8);
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: Some(b),
        color: None,
        fps: None,
        speed: None,
        sound_mode: None,
        reactivity: None,
        sensitivity: None,
    })?;
    Ok(())
}

fn adjust_speed(delta: f32) -> anyhow::Result<()> {
    let resp = ipc_request(&IpcRequest::Status)?;
    let Some(st) = resp.status else {
        return Ok(());
    };
    if !uses_speed(&st.effect) {
        return Ok(());
    }
    let speed = (st.speed + delta).clamp(SPEED_MIN, SPEED_MAX);
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: None,
        color: None,
        fps: None,
        speed: Some(speed),
        sound_mode: None,
        reactivity: None,
        sensitivity: None,
    })?;
    Ok(())
}

fn adjust_reactivity(delta: f32) -> anyhow::Result<()> {
    let resp = ipc_request(&IpcRequest::Status)?;
    let Some(st) = resp.status else {
        return Ok(());
    };
    if st.sound_mode == "off" {
        return Ok(());
    }
    let reactivity = (st.reactivity + delta).clamp(0.0, 1.0);
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: None,
        color: None,
        fps: None,
        speed: None,
        sound_mode: None,
        reactivity: Some(reactivity),
        sensitivity: None,
    })?;
    Ok(())
}

fn adjust_sensitivity(delta: f32) -> anyhow::Result<()> {
    let resp = ipc_request(&IpcRequest::Status)?;
    let Some(st) = resp.status else {
        return Ok(());
    };
    if st.sound_mode == "off" {
        return Ok(());
    }
    let sensitivity = (st.sensitivity + delta).clamp(0.0, 1.0);
    ipc_request(&IpcRequest::Patch {
        mode: None,
        brightness: None,
        color: None,
        fps: None,
        speed: None,
        sound_mode: None,
        reactivity: None,
        sensitivity: Some(sensitivity),
    })?;
    Ok(())
}

fn sound_enabled(state: &UiState) -> bool {
    state.sound_mode != "off"
}

fn audio_separator_line() -> Line<'static> {
    Line::from(Span::styled(
        "── audio ──────────────────",
        Style::default().fg(Color::DarkGray),
    ))
}

fn draw_ui(f: &mut Frame, state: &mut UiState) {
    let root = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(f.area());

    let status_rows = if sound_enabled(state) { 10 } else { 7 };
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_rows),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(root[0]);

    draw_status(f, left[0], state);
    draw_controls_panel(f, left[1], state);
    draw_legend(f, left[2], state);
    draw_effect_list(f, root[1], state);
}

fn draw_status(f: &mut Frame, area: Rect, state: &UiState) {
    let sound_on = sound_enabled(state);
    let status_line = if state.serial_ok {
        format!(
            "OK · {}",
            if state.width > 0 {
                format!("{}x{}", state.width, state.height)
            } else {
                state.detail.clone()
            }
        )
    } else {
        "serial disconnected".into()
    };

    let mut lines = vec![
        Line::from(format!(
            "Brightness  {:.0}% ({:.2})   w/s adjust",
            state.brightness * 100.0,
            state.brightness
        )),
        Line::from(format!("FPS        {}   - = pick", state.fps)),
        Line::from(format!("Active     {}", state.effect)),
    ];
    if sound_on {
        lines.push(audio_separator_line());
        lines.push(Line::from(format!(
            "Sound       {}   Tab cycle",
            state.sound_mode
        )));
        lines.push(Line::from(format!(
            "Boost       {:.0}%   j/k adjust",
            state.reactivity * 100.0
        )));
        lines.push(Line::from(format!(
            "Sensitivity {:.0}%   h/l adjust",
            state.sensitivity * 100.0
        )));
    } else {
        lines.push(Line::from(format!(
            "Sound      {}   Tab cycle",
            state.sound_mode
        )));
    }
    lines.push(Line::from(format!("Status     {status_line}")));
    if let Some(err) = &state.last_error {
        lines.push(Line::from(format!("Error      {err}")));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("status")),
        area,
    );
}

fn draw_controls_panel(f: &mut Frame, area: Rect, state: &UiState) {
    let sound_on = sound_enabled(state);
    let block = Block::default().borders(Borders::ALL).title("controls");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let constraints = [
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("brightness"))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(state.brightness as f64);
    f.render_widget(gauge, chunks[0]);

    let speed_enabled = uses_speed(&state.effect);
    let speed_gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(if speed_enabled {
            "speed"
        } else {
            "speed (animated modes)"
        }))
        .gauge_style(if speed_enabled {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .ratio(((state.speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN)).clamp(0.0, 1.0) as f64)
        .label(format!("{:.1}x", state.speed));
    f.render_widget(speed_gauge, chunks[1]);

    draw_fps_picker(f, chunks[2], state);
    draw_color_picker(f, chunks[3], state);

    f.render_widget(Paragraph::new(audio_separator_line()), chunks[4]);

    let audio_gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(if sound_on {
            "audio level"
        } else {
            "audio level (sound off)"
        }))
        .gauge_style(if sound_on {
            Style::default().fg(Color::Magenta)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .ratio(state.audio_level.clamp(0.0, 1.0) as f64)
        .label(format!("{:.0}%", state.audio_level * 100.0));
    f.render_widget(audio_gauge, chunks[5]);

    let boost_gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("audio brightness boost"),
        )
        .gauge_style(if sound_on {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .ratio(state.reactivity.clamp(0.0, 1.0) as f64)
        .label(format!("{:.0}%", state.reactivity * 100.0));
    f.render_widget(boost_gauge, chunks[6]);

    let sensitivity_gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("audio sensitivity"),
        )
        .gauge_style(if sound_on {
            Style::default().fg(Color::LightBlue)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .ratio(state.sensitivity.clamp(0.0, 1.0) as f64)
        .label(format!("{:.0}%", state.sensitivity * 100.0));
    f.render_widget(sensitivity_gauge, chunks[7]);
}

fn draw_legend(f: &mut Frame, area: Rect, state: &UiState) {
    let legend = if sound_enabled(state) {
        "w/s brightness · a/d speed · - = fps · ↑↓ effect · ←→ color · Tab sound · j/k boost · h/l sensitivity · R reselect"
    } else {
        "w/s brightness · a/d speed · - = fps · ↑↓ effect · ←→ color · Tab sound · R reselect"
    };
    f.render_widget(
        Paragraph::new(legend).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_effect_list(f: &mut Frame, area: Rect, state: &mut UiState) {
    let pick_hint = if state.digit_buf.is_empty() {
        String::new()
    } else {
        format!(" → {}", state.digit_buf)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("effects{pick_hint}"));
    let inner = block.inner(area);
    f.render_widget(block, area);

    ensure_list_cursor_visible(state, inner.height);

    let mut y = inner.y;
    let bottom = inner.y.saturating_add(inner.height);
    let mut idx = state.list_scroll;

    while idx < MODES.len() && y < bottom {
        let mode = MODES[idx];
        let is_active = mode == state.effect.as_str();
        let is_cursor = idx == state.list_cursor;
        let row_h = item_height(idx, &state.effect);
        if y.saturating_add(row_h) > bottom {
            break;
        }

        let row_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: row_h,
        };

        let chevron = if is_cursor { "▶" } else { " " };
        let num = format!("{:2}", idx + 1);
        let label = mode_label(mode);
        let screen_sync = is_screen_sync(mode);
        let line = Line::from(vec![
            Span::styled(
                format!("{chevron} {num} "),
                if is_cursor {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(
                label,
                effect_label_style(is_active, is_cursor, screen_sync),
            ),
        ]);

        if is_active {
            let bordered = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow));
            let inner_row = bordered.inner(row_area);
            f.render_widget(bordered, row_area);
            if inner_row.width > 0 && inner_row.height > 0 {
                f.render_widget(Paragraph::new(line), inner_row);
            }
        } else {
            f.render_widget(Paragraph::new(line), row_area);
        }

        y = y.saturating_add(row_h);
        idx += 1;
    }
}

fn mode_label(mode: &str) -> String {
    match mode {
        "off" => "Off".into(),
        "screen" => "Screen Sync".into(),
        "screen_center" => "Screen Sync Center".into(),
        "solid" => "Solid".into(),
        "candle" => "Candle".into(),
        "chase" => "Chase".into(),
        "wave" => "Wave".into(),
        "scanner" => "Scanner".into(),
        "sparkle" => "Sparkle".into(),
        "pulse" => "Pulse".into(),
        "aurora" => "Aurora".into(),
        "fire" => "Fire".into(),
        "heartbeat" => "Heartbeat".into(),
        "segment" => "Segment".into(),
        "strobe" => "Strobe".into(),
        "wipe" => "Wipe".into(),
        "sound_viz" => "Sound Viz".into(),
        _ => mode.into(),
    }
}

fn is_screen_sync(mode: &str) -> bool {
    mode == "screen" || mode == "screen_center"
}

fn effect_label_style(is_active: bool, is_cursor: bool, screen_sync: bool) -> Style {
    if is_active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else if screen_sync {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else if is_cursor {
        Style::default().fg(Color::White)
    } else {
        Style::default()
    }
}

fn uses_speed(effect: &str) -> bool {
    effect != "off"
        && effect != "solid"
        && effect != "sound_viz"
        && !is_screen_sync(effect)
}

fn uses_accent_color(effect: &str) -> bool {
    effect != "off" && !is_screen_sync(effect)
}

fn color_picker_enabled(effect: &str) -> bool {
    uses_accent_color(effect)
}

fn rainbow_letter_spans(text: &str, selected: bool) -> Vec<Span<'static>> {
    let modifier = if selected {
        Modifier::BOLD | Modifier::UNDERLINED
    } else {
        Modifier::empty()
    };
    let letter_count = text.chars().filter(|c| c.is_ascii_alphabetic()).count().max(1);
    let mut letter_i = 0;
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphabetic() {
                let [r, g, b] = rainbow_pixel(letter_i, letter_count);
                letter_i += 1;
                Span::styled(
                    ch.to_string(),
                    Style::default()
                        .fg(Color::Rgb(r, g, b))
                        .add_modifier(modifier),
                )
            } else {
                Span::raw(ch.to_string())
            }
        })
        .collect()
}

fn draw_fps_picker(f: &mut Frame, area: Rect, state: &UiState) {
    let block = Block::default().borders(Borders::ALL).title("fps");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let preset_line: Vec<Span> = FPS_PRESETS
        .iter()
        .enumerate()
        .flat_map(|(i, &fps)| {
            let selected = i == state.fps_idx;
            let label = if selected {
                format!("[{fps}]")
            } else {
                format!(" {fps} ")
            };
            vec![
                Span::styled(
                    label,
                    Style::default().add_modifier(if selected {
                        Modifier::BOLD | Modifier::UNDERLINED
                    } else {
                        Modifier::empty()
                    }),
                ),
                Span::raw(" "),
            ]
        })
        .collect();

    f.render_widget(
        Paragraph::new(vec![
            Line::from(format!("{} fps  (- = pick)", state.fps)),
            Line::from(preset_line),
        ]),
        inner,
    );
}

fn draw_color_picker(f: &mut Frame, area: Rect, state: &UiState) {
    let enabled = color_picker_enabled(&state.effect);
    let title = if enabled { "color" } else { "color (not used in this mode)" };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    let dim = Style::default().fg(Color::DarkGray);
    f.render_widget(block, area);

    let rgb = if is_rainbow_color(&state.color) {
        rainbow_pixel(2, 8)
    } else {
        parse_color(&state.color).unwrap_or([255, 51, 0])
    };
    let swatch = Rect {
        x: inner.x,
        y: inner.y,
        width: 4,
        height: inner.height.min(3),
    };
    f.render_widget(
        Paragraph::new("    ").style(Style::default().bg(Color::Rgb(rgb[0], rgb[1], rgb[2]))),
        swatch,
    );

    let preset_line: Vec<Span> = COLOR_PRESETS
        .iter()
        .enumerate()
        .flat_map(|(i, preset)| {
            let selected = i == state.color_idx;
            if is_rainbow_color(preset) {
                let label = if selected {
                    "[rainbow]"
                } else {
                    " rainbow "
                };
                rainbow_letter_spans(label, selected)
                    .into_iter()
                    .chain([Span::raw(" ")])
                    .collect::<Vec<_>>()
            } else {
                let c = parse_color(preset).unwrap_or([128, 128, 128]);
                let label = if selected {
                    format!("[#{preset}]")
                } else {
                    format!(" #{preset} ")
                };
                vec![
                    Span::styled(
                        label,
                        Style::default()
                            .fg(Color::Rgb(c[0], c[1], c[2]))
                            .add_modifier(if selected {
                                Modifier::BOLD | Modifier::UNDERLINED
                            } else {
                                Modifier::empty()
                            }),
                    ),
                    Span::raw(" "),
                ]
            }
        })
        .collect();

    let current_line: Line = if enabled && is_rainbow_color(&state.color) {
        Line::from(
            rainbow_letter_spans("rainbow", true)
                .into_iter()
                .chain([Span::raw("  (←→ pick)")])
                .collect::<Vec<_>>(),
        )
    } else {
        Line::from(Span::styled(
            if enabled {
                format!("#{}  (←→ pick)", state.color)
            } else {
                format!("#{}  (not used in this mode)", state.color)
            },
            if enabled {
                Style::default()
            } else {
                dim
            },
        ))
    };

    f.render_widget(
        Paragraph::new(vec![
            current_line,
            Line::from(if enabled {
                preset_line
            } else {
                vec![Span::styled("←→ disabled for this mode", dim)]
            }),
        ]),
        Rect {
            x: inner.x + 5,
            y: inner.y,
            width: inner.width.saturating_sub(5),
            height: inner.height,
        },
    );
}
