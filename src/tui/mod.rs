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

const MODES: &[&str] = &["off", "solid", "candle", "screen"];
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
        }
    }
}

fn tui_loop() -> anyhow::Result<()> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?;
    let mut state = UiState::default();
    let mut last_poll = Instant::now() - Duration::from_secs(1);

    loop {
        // Input first — don't block on IPC before reading keys.
        while event::poll(Duration::from_millis(0))? {
            if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
                match code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Tab => cycle_mode(&state.effect, 1),
                    KeyCode::BackTab => cycle_mode(&state.effect, -1),
                    KeyCode::Char('s') => {
                        patch_mode("solid").ok();
                    }
                    KeyCode::Char('c') => {
                        patch_mode("candle").ok();
                    }
                    KeyCode::Char('y') => {
                        patch_mode("screen").ok();
                    }
                    KeyCode::Char('o') => {
                        patch_mode("off").ok();
                    }
                    KeyCode::Up => {
                        adjust_brightness(0.05).ok();
                    }
                    KeyCode::Down => {
                        adjust_brightness(-0.05).ok();
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

        terminal.draw(|f| draw_ui(f, &state))?;
        thread::sleep(Duration::from_millis(16));
    }
}

fn cycle_mode(current: &str, dir: i32) {
    let idx = MODES.iter().position(|m| *m == current).unwrap_or(0);
    let n = MODES.len() as i32;
    let next = (idx as i32 + dir).rem_euclid(n) as usize;
    patch_mode(MODES[next]).ok();
}

fn cycle_color(state: &mut UiState, dir: i32) {
    if state.effect == "screen" {
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
    if st.effect != "candle" {
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

fn draw_ui(f: &mut Frame, state: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(" hyper-sync ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        effect_tab("Off", &state.effect, "off"),
        Span::raw("  "),
        effect_tab("Solid", &state.effect, "solid"),
        Span::raw("  "),
        effect_tab("Candle", &state.effect, "candle"),
        Span::raw("  "),
        effect_tab("Screen", &state.effect, "screen"),
    ]))
    .block(Block::default().borders(Borders::ALL).title("effect · Tab cycle"));
    f.render_widget(title, chunks[0]);

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
        Line::from(format!("Status     {status_line}")),
    ];
    if let Some(err) = &state.last_error {
        controls.push(Line::from(format!("Error      {err}")));
    }
    f.render_widget(
        Paragraph::new(controls).block(Block::default().borders(Borders::ALL).title("controls")),
        chunks[1],
    );

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("brightness"))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(state.brightness as f64);
    f.render_widget(gauge, chunks[2]);

    let speed_enabled = state.effect == "candle";
    let speed_title = if speed_enabled {
        "speed · [ ] adjust"
    } else {
        "speed (candle only)"
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
    f.render_widget(speed_gauge, chunks[3]);

    draw_color_picker(f, chunks[4], state);

    f.render_widget(
        Paragraph::new("q/Ctrl+C quit · Tab mode · ←→ color · [ ] speed · ↑↓ brightness")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[5],
    );
}

fn color_picker_enabled(effect: &str) -> bool {
    effect == "solid" || effect == "candle"
}

fn draw_color_picker(f: &mut Frame, area: Rect, state: &UiState) {
    let enabled = color_picker_enabled(&state.effect);
    let title = if enabled {
        "color"
    } else {
        "color (solid/candle only)"
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
                    format!("#{}  (solid color · ←→ pick)", state.color)
                } else {
                    format!("#{}  (not used in screen mode)", state.color)
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
                vec![Span::styled("←→ disabled in screen mode", dim)]
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

fn effect_tab(label: &str, current: &str, mode: &str) -> Span<'static> {
    if current == mode {
        Span::styled(
            format!("[{label}]"),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(format!(" {label} "))
    }
}
