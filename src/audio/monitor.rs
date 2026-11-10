use std::io::{ErrorKind, Read};
use std::os::unix::io::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::audio::dynamics::{self, Dynamics};
use crate::audio::envelope::Envelope;
use crate::audio::spectrum::{Analyzer, WINDOW_MAX};
use crate::audio::state::{AudioSnapshot, SPECTRUM_BANDS};
use crate::config::RuntimeConfig;
use crate::daemon::DaemonStatus;

const CHANNELS: usize = 2;
const SAMPLE_RATE: f32 = 48_000.0;
const READ_CHUNK: usize = 2048;
const FRAME_BYTES: usize = CHANNELS * std::mem::size_of::<f32>();
const FRAME_DT: f32 = 1.0 / SAMPLE_RATE;
// ~5 ms buffers: PipeWire then runs the graph in 256-sample cycles instead of 1024 (21 ms).
const PAREC_LATENCY_MS: &str = "5";
const PAREC_PROCESS_MS: &str = "2";
// ponytail: ~100Hz one-pole — hats/treble pass less into brightness boost.
const BASS_LP_ALPHA: f32 = 2.0 * std::f32::consts::PI * 100.0 / SAMPLE_RATE;
const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);

pub struct AudioMonitor {
    snapshot: Arc<AudioSnapshot>,
    status: Arc<Mutex<DaemonStatus>>,
    config: Arc<RwLock<RuntimeConfig>>,
    shutdown: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    handle: Option<JoinHandle<()>>,
}

impl AudioMonitor {
    pub fn new(
        snapshot: Arc<AudioSnapshot>,
        status: Arc<Mutex<DaemonStatus>>,
        config: Arc<RwLock<RuntimeConfig>>,
    ) -> Self {
        Self {
            snapshot,
            status,
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            handle: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    pub fn start(&mut self) {
        if self.is_running() {
            return;
        }
        self.shutdown.store(false, Ordering::SeqCst);
        let snapshot = Arc::clone(&self.snapshot);
        let status = Arc::clone(&self.status);
        let config = Arc::clone(&self.config);
        let shutdown = Arc::clone(&self.shutdown);
        let child = Arc::clone(&self.child);
        self.handle = Some(thread::spawn(move || {
            run_capture_loop(snapshot, status, config, shutdown, child);
        }));
        eprintln!("audio monitor: started");
    }

    pub fn stop(&mut self) {
        if !self.is_running() {
            return;
        }
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(h) = self.handle.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !h.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                eprintln!("audio monitor: stop timed out");
            }
        }
        eprintln!("audio monitor: stopped");
    }
}

fn run_capture_loop(
    snapshot: Arc<AudioSnapshot>,
    status: Arc<Mutex<DaemonStatus>>,
    config: Arc<RwLock<RuntimeConfig>>,
    shutdown: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match run_one_session(
            Arc::clone(&snapshot),
            Arc::clone(&status),
            Arc::clone(&config),
            Arc::clone(&shutdown),
            Arc::clone(&child_slot),
        ) {
            Ok(()) if shutdown.load(Ordering::Relaxed) => break,
            Ok(()) => {
                eprintln!("audio monitor: capture ended — reconnecting in {RECONNECT_BACKOFF:?}");
                snapshot.store_levels(0.0, 0.0, 0.0, 0.0);
            }
            Err(e) => {
                eprintln!("audio monitor: {e:#} — retry in {RECONNECT_BACKOFF:?}");
                status.lock().unwrap().last_error = Some(format!("audio: {e:#}"));
                snapshot.store_levels(0.0, 0.0, 0.0, 0.0);
            }
        }
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        // Interruptible backoff so stop() returns quickly.
        let deadline = Instant::now() + RECONNECT_BACKOFF;
        while Instant::now() < deadline && !shutdown.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn default_sink_name() -> Option<String> {
    let output = Command::new("pactl").arg("get-default-sink").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn monitor_device(sink: &str) -> String {
    if sink.ends_with(".monitor") {
        sink.to_string()
    } else {
        format!("{sink}.monitor")
    }
}

fn set_nonblocking(fd: i32) -> anyhow::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    anyhow::ensure!(flags >= 0, "fcntl F_GETFL");
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    anyhow::ensure!(rc == 0, "fcntl F_SETFL O_NONBLOCK");
    Ok(())
}

struct CaptureState {
    level_env: Envelope,
    bass_env: Envelope,
    left_env: Envelope,
    right_env: Envelope,
    lp_mono: f32,
    spec_env: [Envelope; SPECTRUM_BANDS],
    analyzer: Analyzer,
    /// Newest signed mono samples, enough for the longest spectrum window.
    history: Vec<f32>,
    last_spectrum: Instant,
    spectrum: [f32; SPECTRUM_BANDS],
    logged_first_buffer: bool,
    logged_peak: bool,
}

fn run_one_session(
    snapshot: Arc<AudioSnapshot>,
    status: Arc<Mutex<DaemonStatus>>,
    config: Arc<RwLock<RuntimeConfig>>,
    shutdown: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
) -> anyhow::Result<()> {
    let sink_name = default_sink_name().context("default audio sink (pactl)")?;
    let device = monitor_device(&sink_name);
    eprintln!("audio monitor: parec {device}");

    let mut child = Command::new("parec")
        .args([
            "-d",
            &device,
            "--format=float32le",
            "--channels=2",
            "--rate=48000",
            "--latency-msec",
            PAREC_LATENCY_MS,
            "--process-time-msec",
            PAREC_PROCESS_MS,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("parec (install pulseaudio-utils)")?;

    let mut stdout = child.stdout.take().context("parec stdout")?;
    set_nonblocking(stdout.as_raw_fd())?;
    *child_slot.lock().unwrap() = Some(child);

    status.lock().unwrap().last_error = None;

    let init = { let a = &config.read().unwrap().audio; dynamics::from_settings(a.sensitivity, a.smoothing) };
    let mut state = CaptureState {
        level_env: Envelope::new(init.attack_ms, init.release_ms),
        bass_env: Envelope::new(init.attack_ms, init.release_ms),
        left_env: Envelope::new(init.attack_ms, init.release_ms),
        right_env: Envelope::new(init.attack_ms, init.release_ms),
        lp_mono: 0.0,
        spec_env: std::array::from_fn(|_| Envelope::new(10.0, 120.0)),
        analyzer: Analyzer::new(SAMPLE_RATE),
        history: Vec::with_capacity(WINDOW_MAX * 2),
        last_spectrum: Instant::now(),
        spectrum: [0.0; SPECTRUM_BANDS],
        logged_first_buffer: false,
        logged_peak: false,
    };

    let mut buf = vec![0u8; READ_CHUNK];
    let mut carry = Vec::new();
    let mut idle_since = Instant::now();

    while !shutdown.load(Ordering::Relaxed) {
        // PipeWire restart: parec may exit or hang with no data.
        if let Some(child) = child_slot.lock().unwrap().as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                eprintln!("audio monitor: parec exited ({status})");
                break;
            }
        }

        match stdout.read(&mut buf) {
            Ok(0) => {
                eprintln!("audio monitor: parec EOF");
                break;
            }
            Ok(n) => {
                idle_since = Instant::now();
                carry.extend_from_slice(&buf[..n]);
                let usable = carry.len() / FRAME_BYTES * FRAME_BYTES;
                if usable < FRAME_BYTES {
                    continue;
                }
                let dyn_cfg = {
                    let a = &config.read().unwrap().audio;
                    dynamics::from_settings(a.sensitivity, a.smoothing)
                };
                process_chunk(&carry[..usable], &mut state, &snapshot, dyn_cfg);
                carry.drain(..usable);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                // No audio for a while after PipeWire blip — drop meters, keep waiting briefly.
                if idle_since.elapsed() > Duration::from_secs(5) {
                    snapshot.store_levels(0.0, 0.0, 0.0, 0.0);
                    idle_since = Instant::now();
                }
                // Wake the moment parec writes (not up to 20 ms later); time out to recheck parec.
                let mut pfd = libc::pollfd { fd: stdout.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                unsafe { libc::poll(&mut pfd, 1, 100) };
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }

    if let Some(mut child) = child_slot.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

fn process_chunk(
    bytes: &[u8],
    state: &mut CaptureState,
    snapshot: &AudioSnapshot,
    dyn_cfg: Dynamics,
) {
    if !state.logged_first_buffer {
        state.logged_first_buffer = true;
        eprintln!("audio monitor: first buffer ({} bytes)", bytes.len());
    }
    if bytes.len() < FRAME_BYTES {
        return;
    }

    state.level_env.set_timing(dyn_cfg.attack_ms, dyn_cfg.release_ms);
    state.bass_env.set_timing(dyn_cfg.attack_ms, dyn_cfg.release_ms);
    state.left_env.set_timing(dyn_cfg.attack_ms, dyn_cfg.release_ms);
    state.right_env.set_timing(dyn_cfg.attack_ms, dyn_cfg.release_ms);

    let block_samples = dyn_cfg.block_samples.max(64);
    let block_dt = block_samples as f32 * FRAME_DT;

    let mut l_peak = 0.0f32;
    let mut r_peak = 0.0f32;
    let mut mono_buf = Vec::with_capacity(bytes.len() / FRAME_BYTES);
    let mut block_l = 0.0f32;
    let mut block_r = 0.0f32;
    let mut block_mono = 0.0f32;
    let mut block_bass = 0.0f32;
    let mut block_n = 0usize;

    for chunk in bytes.chunks_exact(FRAME_BYTES) {
        let l = sample_f32(&chunk[0..4]);
        let r = sample_f32(&chunk[4..8]);
        l_peak = l_peak.max(l.abs());
        r_peak = r_peak.max(r.abs());
        let mono = (l + r) * 0.5;
        mono_buf.push(mono);

        state.lp_mono += BASS_LP_ALPHA * (mono - state.lp_mono);
        let bass = state.lp_mono.abs();

        block_l = block_l.max(l.abs());
        block_r = block_r.max(r.abs());
        block_mono = block_mono.max(mono.abs());
        block_bass = block_bass.max(bass);
        block_n += 1;
        if block_n >= block_samples {
            state
                .level_env
                .tick(dynamics::meter_gain(block_mono, dyn_cfg.drive), block_dt);
            state
                .bass_env
                .tick(dynamics::meter_gain(block_bass, dyn_cfg.drive), block_dt);
            state
                .left_env
                .tick(dynamics::meter_gain(block_l, dyn_cfg.drive), block_dt);
            state
                .right_env
                .tick(dynamics::meter_gain(block_r, dyn_cfg.drive), block_dt);
            block_l = 0.0;
            block_r = 0.0;
            block_mono = 0.0;
            block_bass = 0.0;
            block_n = 0;
        }
    }
    if block_n > 0 {
        let dt = block_n as f32 * FRAME_DT;
        state
            .level_env
            .tick(dynamics::meter_gain(block_mono, dyn_cfg.drive), dt);
        state
            .bass_env
            .tick(dynamics::meter_gain(block_bass, dyn_cfg.drive), dt);
        state
            .left_env
            .tick(dynamics::meter_gain(block_l, dyn_cfg.drive), dt);
        state
            .right_env
            .tick(dynamics::meter_gain(block_r, dyn_cfg.drive), dt);
    }

    if !state.logged_peak && (l_peak > 1e-4 || r_peak > 1e-4) {
        state.logged_peak = true;
        eprintln!("audio monitor: peak L={l_peak:.4} R={r_peak:.4}");
    }
    snapshot.store_levels(
        state.level_env.value(),
        state.bass_env.value(),
        state.left_env.value(),
        state.right_env.value(),
    );

    // Spectrum on every chunk (~5 ms) from a rolling window, not on 5 ms snippets every 100 ms.
    state.history.extend_from_slice(&mono_buf);
    let excess = state.history.len().saturating_sub(WINDOW_MAX);
    state.history.drain(..excess);
    if !mono_buf.is_empty() {
        let spec_dt = state.last_spectrum.elapsed().as_secs_f32().max(1e-4);
        state.last_spectrum = Instant::now();
        let bands = state.analyzer.analyze(&state.history);
        for (i, &b) in bands.iter().enumerate() {
            // Sound Viz bars fall back as slowly as the smoothing setting says.
            state.spec_env[i].set_timing(4.0, dyn_cfg.release_ms * 1.5);
            state.spec_env[i].tick(b, spec_dt);
            state.spectrum[i] = state.spec_env[i].value();
        }
        snapshot.store_spectrum(&state.spectrum);
    }
}

fn sample_f32(bytes: &[u8]) -> f32 {
    // Signed: the bass filter and the spectrum need the waveform, not its absolute value.
    let v = f32::from_le_bytes(bytes.try_into().unwrap());
    if v.is_finite() {
        v.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}
