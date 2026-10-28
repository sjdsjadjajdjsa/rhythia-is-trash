use std::time::Instant;

use serde_json::{Value, json};

use crate::audio::ClockStats;

const RECENT: usize = 240;
const KEEP: usize = 60_000;

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub avg_ms: f32,
    pub p99_ms: f32,
    pub p50_ms: f32,
    pub delay_ms: f32,
    pub swap_ms: f32,
}

pub struct Perf {
    last: Instant,
    recent: Vec<f32>,
    swap_recent: Vec<f32>,
    swaps: Vec<f32>,
    delays: Vec<f32>,
    awaits: Vec<f32>,
    frames: Vec<f32>,
    cpu: Vec<f32>,
    updates: Vec<f32>,
    cached: Stats,
    since_cache: u32,
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    sorted
        .get(((sorted.len() as f32 - 1.0) * p).round() as usize)
        .copied()
        .unwrap_or(0.0)
}

fn summary(values: &[f32]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    json!({
        "avg": sorted.iter().sum::<f32>() / sorted.len().max(1) as f32,
        "p50": percentile(&sorted, 0.5),
        "p99": percentile(&sorted, 0.99),
        "p999": percentile(&sorted, 0.999),
        "max": sorted.last().copied().unwrap_or(0.0),
    })
}

impl Perf {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            recent: Vec::with_capacity(RECENT),
            swap_recent: Vec::new(),
            swaps: Vec::new(),
            delays: Vec::new(),
            awaits: Vec::new(),
            frames: Vec::new(),
            cpu: Vec::new(),
            updates: Vec::new(),
            cached: Stats::default(),
            since_cache: 0,
        }
    }

    pub fn tick(&mut self) -> f32 {
        let ms = self.last.elapsed().as_secs_f32() * 1000.0;
        self.last = Instant::now();
        if self.recent.len() == RECENT {
            self.recent.remove(0);
        }
        self.recent.push(ms);
        if self.frames.len() < KEEP {
            self.frames.push(ms);
        }
        self.since_cache += 1;
        if self.since_cache >= 15 {
            self.since_cache = 0;
            let mut sorted = self.recent.clone();
            sorted.sort_by(f32::total_cmp);
            let avg = sorted.iter().sum::<f32>() / sorted.len() as f32;
            self.cached = Stats {
                avg_ms: avg,
                p99_ms: percentile(&sorted, 0.99),
                p50_ms: percentile(&sorted, 0.5),
                delay_ms: self.cached.delay_ms,
                swap_ms: self.swap_recent.iter().sum::<f32>()
                    / self.swap_recent.len().max(1) as f32,
            };
        }
        ms
    }

    pub fn swap(&mut self, wait_ms: f32, delay_ms: f32) {
        let ms = (wait_ms - delay_ms).max(0.0);
        if self.awaits.len() < KEEP {
            self.awaits.push(wait_ms);
            self.delays.push(delay_ms);
        }
        if self.swap_recent.len() == RECENT {
            self.swap_recent.remove(0);
        }
        self.swap_recent.push(ms);
        if self.swaps.len() < KEEP {
            self.swaps.push(ms);
        }
    }

    pub fn cpu(&mut self, started: Instant) {
        if self.cpu.len() < KEEP {
            self.cpu.push(started.elapsed().as_secs_f32() * 1000.0);
        }
    }

    pub fn update(&mut self, ms: f32) {
        if self.updates.len() < KEEP {
            self.updates.push(ms);
        }
    }

    pub fn stats(&self) -> Stats {
        self.cached
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.cpu.clear();
        self.updates.clear();
        self.swaps.clear();
        self.awaits.clear();
        self.delays.clear();
    }

    pub fn json(&self, clock: ClockStats) -> Value {
        let mut sorted = self.frames.clone();
        sorted.sort_by(f32::total_cmp);
        let median = percentile(&sorted, 0.5);
        json!({
            "frames": self.frames.len(),
            "frame_ms": summary(&self.frames),
            "cpu_ms": summary(&self.cpu),
            "update_ms": summary(&self.updates),
            "swap_ms": summary(&self.swaps),
            "slow_frames": self.frames.iter().filter(|f| **f > median * 1.5).count(),
            "pacer_delay_ms": summary(&self.delays),
            "pacer_zero_pct": 100.0 * self.delays.iter().filter(|d| **d < 0.05).count() as f32
                / self.delays.len().max(1) as f32,
            "audio_period_ms": clock.period_ms,
            "clock_jitter_ms": clock.jitter_ms,
        })
    }

    pub fn report(&self, clock: ClockStats) -> String {
        let mut frames = self.frames.clone();
        frames.sort_by(f32::total_cmp);
        let mut swaps = self.swaps.clone();
        swaps.sort_by(f32::total_cmp);
        let avg = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        let max = |v: &[f32]| v.iter().copied().fold(0.0, f32::max);
        format!(
            "frames {} | await ms avg {:.2} delay avg {:.2} swap block ms avg {:.2} p99 {:.2} | frame ms avg {:.2} p50 {:.2} p99 {:.2} max {:.2} | cpu ms avg {:.2} max {:.2} | audio period {:.2} ms | detect spread {:.2} ms | clock jitter {:.3} ms rms",
            frames.len(),
            avg(&self.awaits),
            avg(&self.delays),
            avg(&swaps),
            percentile(&swaps, 0.99),
            avg(&frames),
            percentile(&frames, 0.5),
            percentile(&frames, 0.99),
            max(&frames),
            avg(&self.cpu),
            max(&self.cpu),
            clock.period_ms,
            clock.spread_ms,
            clock.jitter_ms,
        )
    }
}

const WARMUP_FRAMES: u32 = 30;
const SAFETY_MS: f32 = 2.5;
const STEP_MS: f32 = 0.5;
const STEP_FRAMES: u32 = 10;
const BACKOFF_MS: f32 = 1.0;
// The cap only ever dropped, so one burst of load (another GPU app, busy cores) left the
// pacer at 0 ms for the rest of the session. After RECOVER_MS without a miss the cap rises
// by RECOVER_STEP_MS; a miss within PROBE_MS of a raise doubles the wait (up to 8x).
const RECOVER_MS: f32 = 4000.0;
const RECOVER_STEP_MS: f32 = 1.0;
const PROBE_MS: f32 = 2000.0;
const MAX_WAIT_MULT: u32 = 8;

pub struct Pacer {
    delay_ms: f32,
    cap_ms: f32,
    calm: u32,
    warmup: u32,
    quiet: u32,
    wait_mult: u32,
    probing: bool,
    pub reached_ms: f32,
    pub misses: u32,
}

impl Pacer {
    pub fn new() -> Self {
        Self {
            delay_ms: 0.0,
            cap_ms: f32::MAX,
            calm: 0,
            warmup: 0,
            quiet: 0,
            wait_mult: 1,
            probing: false,
            reached_ms: 0.0,
            misses: 0,
        }
    }

    pub fn delay_ms(&self) -> f32 {
        self.delay_ms
    }

    pub fn cap_ms(&self) -> Option<f32> {
        (self.cap_ms < f32::MAX).then_some(self.cap_ms)
    }

    pub fn update(&mut self, active: bool, frame_ms: f32, interval_ms: f32) {
        if !active {
            self.delay_ms = 0.0;
            self.calm = 0;
            self.warmup = 0;
            self.quiet = 0;
            return;
        }
        self.warmup += 1;
        if self.warmup < WARMUP_FRAMES || interval_ms <= 0.0 {
            return;
        }
        let top = (interval_ms - SAFETY_MS).max(0.0);
        let ceiling = top.min(self.cap_ms);
        self.quiet += 1;
        if frame_ms > interval_ms * 1.8 && self.delay_ms > 0.0 {
            self.misses += 1;
            if self.probing {
                self.wait_mult = (self.wait_mult * 2).min(MAX_WAIT_MULT);
            }
            self.probing = false;
            self.cap_ms = (self.delay_ms - BACKOFF_MS).max(0.0);
            self.delay_ms = self.cap_ms;
            self.calm = 0;
            self.quiet = 0;
        } else if self.delay_ms < ceiling {
            self.calm += 1;
            if self.calm >= STEP_FRAMES {
                self.calm = 0;
                self.delay_ms = (self.delay_ms + STEP_MS).min(ceiling);
            }
        } else {
            self.delay_ms = ceiling;
        }
        let frames = |ms: f32| (ms / interval_ms).ceil() as u32;
        if self.cap_ms < top && self.quiet >= frames(RECOVER_MS) * self.wait_mult {
            self.cap_ms += RECOVER_STEP_MS;
            if self.cap_ms >= top {
                self.cap_ms = f32::MAX;
            }
            self.quiet = 0;
            self.probing = true;
        } else if self.probing && self.quiet >= frames(PROBE_MS) {
            self.probing = false;
            self.wait_mult = 1;
        }
        self.reached_ms = self.reached_ms.max(self.delay_ms);
    }
}

/// What a frame-rate setting asks of the loop. Every cap is a sleep before input is read
/// (miniquad::window::set_frame_delay), so a capped frame still samples input as late as it can.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cap {
    /// No vsync, no sleep.
    Unlimited,
    /// Vsync paces the loop; while playing, the low-latency Pacer adds its own sleep.
    Vsync,
    /// Sleep-limited to this rate.
    Fps(f32),
    /// A fixed sleep in ms (the old menu behaviour: 11.5 ms, about 72 fps on a 144 Hz screen).
    Delay(f32),
}

impl Cap {
    pub fn label(self) -> String {
        match self {
            Cap::Unlimited => "unlimited".into(),
            Cap::Vsync => "vsync".into(),
            Cap::Fps(f) => format!("{f:.0}"),
            Cap::Delay(ms) => format!("{ms} ms sleep"),
        }
    }
}

/// The fps counter: frames counted over half-second windows, so it follows a cap change
/// (or the 15 fps background rate) within a second whatever the frame rate.
#[derive(Default)]
pub struct FpsMeter {
    window_s: f64,
    frames: u32,
    fps: f32,
}

impl FpsMeter {
    const WINDOW_S: f64 = 0.5;

    /// One frame of `frame_ms`; returns the latest whole-window rate.
    pub fn tick(&mut self, frame_ms: f32) -> f32 {
        self.window_s += f64::from(frame_ms) / 1000.0;
        self.frames += 1;
        if self.window_s >= Self::WINDOW_S {
            self.fps = (f64::from(self.frames) / self.window_s) as f32;
            self.window_s = 0.0;
            self.frames = 0;
        }
        self.fps
    }
}

/// How hard one frame's error moves the limiter's sleep (integral control; the sleep set
/// now lands in the next frame, so anything near 1 would ring).
const LIMIT_GAIN: f32 = 0.3;
/// With vsync on, aim a little under the period: a cap equal to the refresh rate would
/// otherwise keep pushing the sleep into a missed refresh.
const VSYNC_SLACK: f32 = 0.98;

/// Turns a `Cap` into this frame's sleep before input.
#[derive(Default)]
pub struct Limiter {
    delay_ms: f32,
}

impl Limiter {
    /// `frame_ms`: the last full frame (it includes the previous sleep). `pacer_ms`: the
    /// low-latency Pacer's sleep, used as is under vsync.
    pub fn delay_ms(&mut self, cap: Cap, frame_ms: f32, vsync: bool, pacer_ms: f32) -> f32 {
        match cap {
            Cap::Unlimited => {
                self.delay_ms = 0.0;
                0.0
            }
            Cap::Vsync => {
                self.delay_ms = 0.0;
                pacer_ms
            }
            Cap::Delay(ms) => {
                self.delay_ms = 0.0;
                ms.max(0.0)
            }
            Cap::Fps(fps) => {
                let period = 1000.0 / fps.max(1.0);
                let target = if vsync { period * VSYNC_SLACK } else { period };
                // A hitch (loading, a dragged window) must not wipe the sleep out for seconds.
                let frame = frame_ms.clamp(0.0, period * 2.0);
                self.delay_ms = (self.delay_ms + LIMIT_GAIN * (target - frame)).clamp(0.0, period);
                self.delay_ms
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: f32 = 6.94;

    /// Runs the limiter against a loop whose frame = work + sleep (+ vsync rounding).
    fn settle(cap: Cap, work_ms: f32, refresh_ms: Option<f32>) -> (f32, f32) {
        let mut l = Limiter::default();
        let (mut delay, mut sum) = (0.0, 0.0);
        let frames = 2000;
        for i in 0..frames {
            let raw = work_ms + delay;
            let frame = refresh_ms.map_or(raw, |r| (raw / r).ceil().max(1.0) * r);
            if i >= frames - 500 {
                sum += frame;
            }
            delay = l.delay_ms(cap, frame, refresh_ms.is_some(), 1.25);
        }
        (delay, sum / 500.0)
    }

    #[test]
    fn caps_map_to_the_right_sleep() {
        for fps in [60.0, 120.0, 144.0, 240.0, 360.0] {
            let (delay, avg) = settle(Cap::Fps(fps), 1.5, None);
            let period = 1000.0 / fps;
            assert!((avg - period).abs() < 0.05, "{fps}: frame {avg}");
            assert!(
                (delay - (period - 1.5)).abs() < 0.05,
                "{fps}: delay {delay}"
            );
        }
        // work slower than the cap: never sleeps
        assert_eq!(settle(Cap::Fps(360.0), 4.0, None).0, 0.0);
        // vsync at 144 Hz with a 60 cap: averages near (a little over) 60
        let (_, avg) = settle(Cap::Fps(60.0), 1.5, Some(INTERVAL));
        assert!(avg > 15.9 && avg < 16.8, "{avg}");
        // a cap at the refresh rate under vsync settles at zero sleep, no missed refreshes
        let (delay, avg) = settle(Cap::Fps(144.0), 1.5, Some(INTERVAL));
        assert!(
            delay < 0.5 && (avg - INTERVAL).abs() < 1e-3,
            "{delay} {avg}"
        );
        let mut l = Limiter::default();
        assert_eq!(l.delay_ms(Cap::Unlimited, 1.0, false, 3.0), 0.0);
        assert_eq!(l.delay_ms(Cap::Vsync, 6.9, true, 3.0), 3.0);
        assert_eq!(l.delay_ms(Cap::Delay(11.5), 13.9, true, 3.0), 11.5);
        // menus at 15 fps when unfocused
        let (delay, _) = settle(Cap::Fps(15.0), 2.0, None);
        assert!((delay - (1000.0 / 15.0 - 2.0)).abs() < 0.1);
        // a hitch moves the sleep by at most one period's worth of error
        let mut l = Limiter::default();
        let before = (0..400).fold(0.0, |d, _| l.delay_ms(Cap::Fps(60.0), 1.5 + d, false, 0.0));
        let after = l.delay_ms(Cap::Fps(60.0), 500.0, false, 0.0);
        assert!(before - after < 1000.0 / 60.0 * LIMIT_GAIN + 1e-3);
    }

    #[test]
    fn fps_meter_follows_a_rate_change_within_a_second() {
        let mut m = FpsMeter::default();
        let mut fps = 0.0;
        for _ in 0..300 {
            fps = m.tick(1000.0 / 144.0);
        }
        assert!((fps - 144.0).abs() < 1.0, "{fps}");
        for _ in 0..15 {
            fps = m.tick(1000.0 / 15.0);
        }
        assert!((fps - 15.0).abs() < 0.5, "{fps}");
    }

    fn run(p: &mut Pacer, frames: u32, frame_ms: f32) {
        for _ in 0..frames {
            p.update(true, frame_ms, INTERVAL);
        }
    }

    fn collapsed() -> Pacer {
        let mut p = Pacer::new();
        run(&mut p, 400, INTERVAL);
        assert!((p.delay_ms() - (INTERVAL - SAFETY_MS)).abs() < 0.01);
        for _ in 0..8 {
            p.update(true, INTERVAL * 2.0, INTERVAL);
            run(&mut p, 3, INTERVAL);
        }
        assert_eq!(p.delay_ms(), 0.0);
        assert_eq!(p.cap_ms(), Some(0.0));
        p
    }

    #[test]
    fn climbs_to_interval_minus_safety() {
        let mut p = Pacer::new();
        run(&mut p, 400, INTERVAL);
        assert!((p.delay_ms() - 4.44).abs() < 0.01);
    }

    #[test]
    fn miss_lowers_cap_and_delay() {
        let mut p = Pacer::new();
        run(&mut p, 400, INTERVAL);
        p.update(true, INTERVAL * 2.0, INTERVAL);
        assert_eq!(p.misses, 1);
        assert!((p.delay_ms() - 3.44).abs() < 0.01);
        run(&mut p, 100, INTERVAL);
        assert!(p.delay_ms() <= 3.44 + 0.01);
    }

    #[test]
    fn cap_recovers_after_calm_seconds() {
        let mut p = collapsed();
        run(&mut p, 400, INTERVAL);
        assert_eq!(p.delay_ms(), 0.0, "no recovery before the calm period");
        // 5 raises of 1 ms at 4 s each, plus the 0.5 ms delay ramp per 10 frames.
        run(&mut p, 5 * 600, INTERVAL);
        assert!(p.cap_ms().is_none());
        assert!((p.delay_ms() - (INTERVAL - SAFETY_MS)).abs() < 0.01);
    }

    #[test]
    fn delay_never_exceeds_interval_minus_safety() {
        let mut p = collapsed();
        for _ in 0..20_000 {
            p.update(true, INTERVAL, INTERVAL);
            assert!(p.delay_ms() <= INTERVAL - SAFETY_MS + 1e-4);
        }
    }

    #[test]
    fn miss_right_after_a_raise_backs_off_longer() {
        let mut p = collapsed();
        run(&mut p, 600, INTERVAL);
        // The first raise lifts the cap to 1 ms; a miss shortly after doubles the wait.
        let mut raised = false;
        for _ in 0..600 {
            p.update(true, INTERVAL, INTERVAL);
            if p.cap_ms() == Some(1.0) {
                raised = true;
                break;
            }
        }
        assert!(raised);
        run(&mut p, 20, INTERVAL);
        assert!(p.delay_ms() > 0.0);
        p.update(true, INTERVAL * 2.0, INTERVAL);
        assert_eq!(p.cap_ms(), Some(0.0));
        run(&mut p, 600 + 100, INTERVAL);
        assert_eq!(p.cap_ms(), Some(0.0), "wait doubled to 8 s");
        run(&mut p, 600, INTERVAL);
        assert_eq!(p.cap_ms(), Some(1.0));
    }

    #[test]
    fn inactive_resets_delay_but_not_cap() {
        let mut p = collapsed();
        p.update(false, INTERVAL, INTERVAL);
        assert_eq!(p.cap_ms(), Some(0.0));
        assert_eq!(p.delay_ms(), 0.0);
    }
}
