use std::io::stdout;
use std::process::Stdio;
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
use crate::effects::solid::parse_color;

const MODES: &[&str] = &[
    "off",
    "screen",
    "screen_center",
    "solid",
    "candle",
    "chase",
    "wave",
    "rainbow",
    "scanner",
    "sparkle",
    "pulse",
    "aurora",
    "fire",
    "heartbeat",
    "segment",
    "strobe",
    "wipe",
];
const COLOR_PRESETS: &[&str] = &[
    "ff3300", "ff0000", "ffffff", "0099ff", "00ff88", "ff00ff", "ffff00", "000000",
];
const SPEED_MIN: f32 = 0.1;
const SPEED_MAX: f32 = 5.0;

pub fn run() -> anyhow::Result<()> {
    ensure_daemon()?;

    enable_raw_mode().context("raw mode")?;
    stdout().execute(EnterAlternateScreen).context("alt screen")?;

    let result = tui_loop();

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
    color_idx: usize,
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
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            effect: "screen".into(),
            brightness: 0.8,
            fps: 30,
            speed: 1.0,
            detail: String::new(),
            color: "ff3300".into(),
            width: 0,
            height: 0,
            serial_ok: true,
            last_error: None,
            color_idx: 0,
            list_cursor: 1,
            digit_buf: String::new(),
            list_scroll: 0,
        }
    }
}

fn tui_loop() -> anyhow::Result<()> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?;
    let mut state = UiState::default();
    let mut last_poll = Instant::now() - Duration::from_secs(1);

    loop {
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
                    KeyCode::Up => {
                        adjust_brightness(0.05).ok();
                    }
                    KeyCode::Down => {
                        adjust_brightness(-0.05).ok();
                    }
                    KeyCode::Char('k') => move_list_cursor(&mut state, -1),
                    KeyCode::Char('j') => move_list_cursor(&mut state, 1),
                    KeyCode::Enter | KeyCode::Char(' ') => select_effect(&mut state),
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        state.digit_buf.push(c);
                        if state.digit_buf.len() >= 2 {
                            select_effect(&mut state);
                        }
                    }
                    KeyCode::Left => cycle_color(&mut state, -1),
                    KeyCode::Right => cycle_color(&mut state, 1),
                    KeyCode::Char('[') => {
                        adjust_speed(-0.1).ok();
                    }
                    KeyCode::Char(']') => {
                        adjust_speed(0.1).ok();
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
                    state.sync_color_idx();
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
    state.list_cursor = (state.list_cursor as i32 + dir).rem_euclid(n) as usize;
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

fn select_effect(state: &mut UiState) {
    let idx = if !state.digit_buf.is_empty() {
        state
            .digit_buf
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&i| i < MODES.len())
            .unwrap_or(state.list_cursor)
    } else {
        state.list_cursor
    };
    state.digit_buf.clear();
    state.list_cursor = idx;
    patch_mode(MODES[idx]).ok();
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
    })
    .ok();
}

fn patch_mode(mode: &str) -> anyhow::Result<()> {
    ipc_request(&IpcRequest::Patch {
        mode: Some(mode.into()),
        brightness: None,
        color: None,
        fps: None,
        speed: None,
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
    })?;
    Ok(())
}

fn draw_ui(f: &mut Frame, state: &mut UiState) {
    let root = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(f.area());

    draw_controls(f, root[0], state);
    draw_effect_list(f, root[1], state);
}

fn draw_controls(f: &mut Frame, area: Rect, state: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

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

    let mut controls = vec![
        Line::from(format!("Brightness  {:.2}   ↑↓ adjust", state.brightness)),
        Line::from(format!("FPS        {}", state.fps)),
        Line::from(format!("Active     {}", state.effect)),
        Line::from(format!("Status     {status_line}")),
    ];
    if let Some(err) = &state.last_error {
        controls.push(Line::from(format!("Error      {err}")));
    }
    f.render_widget(
        Paragraph::new(controls).block(Block::default().borders(Borders::ALL).title("controls")),
        chunks[0],
    );

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("brightness"))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(state.brightness as f64);
    f.render_widget(gauge, chunks[1]);

    let speed_enabled = uses_speed(&state.effect);
    let speed_title = if speed_enabled {
        "speed · [ ] adjust"
    } else {
        "speed (animated modes)"
    };
    let speed_gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(speed_title))
        .gauge_style(if speed_enabled {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .ratio(((state.speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN)).clamp(0.0, 1.0) as f64)
        .label(format!("{:.1}x", state.speed));
    f.render_widget(speed_gauge, chunks[2]);

    draw_color_picker(f, chunks[3], state);

    f.render_widget(
        Paragraph::new("↑↓ brightness · ←→ color · [ ] speed · j/k select · #+Enter effect")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[5],
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
        "rainbow" => "Rainbow".into(),
        "scanner" => "Scanner".into(),
        "sparkle" => "Sparkle".into(),
        "pulse" => "Pulse".into(),
        "aurora" => "Aurora".into(),
        "fire" => "Fire".into(),
        "heartbeat" => "Heartbeat".into(),
        "segment" => "Segment".into(),
        "strobe" => "Strobe".into(),
        "wipe" => "Wipe".into(),
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
    effect != "off" && effect != "solid" && !is_screen_sync(effect)
}

fn uses_accent_color(effect: &str) -> bool {
    effect != "off" && !is_screen_sync(effect) && effect != "rainbow"
}

fn color_picker_enabled(effect: &str) -> bool {
    uses_accent_color(effect)
}

fn draw_color_picker(f: &mut Frame, area: Rect, state: &UiState) {
    let enabled = color_picker_enabled(&state.effect);
    let title = if enabled {
        "color"
    } else if state.effect == "rainbow" {
        "color (auto hue in rainbow)"
    } else {
        "color (not used in this mode)"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    let dim = Style::default().fg(Color::DarkGray);
    f.render_widget(block, area);

    let rgb = parse_color(&state.color).unwrap_or([255, 51, 0]);
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
        .flat_map(|(i, hex)| {
            let c = parse_color(hex).unwrap_or([128, 128, 128]);
            let label = if i == state.color_idx {
                format!("[#{hex}]")
            } else {
                format!(" #{hex} ")
            };
            [
                Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Rgb(c[0], c[1], c[2]))
                        .add_modifier(if i == state.color_idx {
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
            Line::from(Span::styled(
                if enabled {
                    format!("#{}  (←→ pick)", state.color)
                } else if state.effect == "rainbow" {
                    format!("#{}  (rainbow ignores color)", state.color)
                } else {
                    format!("#{}  (not used in this mode)", state.color)
                },
                if enabled {
                    Style::default()
                } else {
                    dim
                },
            )),
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
