//! Live preview of the game inside the settings screen: a `Game` on autoplay that loops
//! a short window of a song and mirrors the settings being edited.

use kira::sound::static_sound::StaticSoundData;
use macroquad::prelude::*;

use crate::{
    audio::{self, Music},
    demo,
    game::Game,
    map::{Map, Note},
    postfx::PostFx,
    settings::Settings,
    skin::Skin,
};

/// Length of the looped stretch, seconds of song time.
const WINDOW: f64 = 12.0;
/// Notes counted when looking for the first dense stretch.
const DENSE_SPAN: f64 = 4.0;
/// Lead-in before the dense stretch (on top of the approach time).
const RUN_UP: f64 = 0.75;
/// Minimum gap between `Music::set_volumes` calls while a volume slider is dragged.
const VOLUME_EVERY: f64 = 0.05;

pub struct Preview {
    game: Game,
    fx: PostFx,
    out: Option<RenderTarget>,
    window: (f64, f64),
    volumes: (f32, f32),
    volumes_at: f64,
    hidden: bool,
    began: Option<std::time::Instant>,
    cost: (f64, u32),
    /// When the game was last rendered into `out` (the preview frame-rate setting).
    drawn_at: f64,
}

/// Whether the preview panel re-renders this frame at `rate` frames per second (None = always).
fn due(now: f64, drawn_at: f64, rate: Option<f64>) -> bool {
    // 2 ms early is fine: a 60 fps preview on a 60 Hz screen must not skip every other frame.
    rate.is_none_or(|r| now - drawn_at >= 1.0 / r.max(1.0) - 0.002)
}

impl Preview {
    /// The built-in demo song (cached after the first build, so reopening settings is cheap).
    pub fn demo(settings: &Settings) -> Preview {
        thread_local! {
            static DEMO: std::cell::OnceCell<(Vec<Note>, String, StaticSoundData)> =
                const { std::cell::OnceCell::new() };
        }
        let (notes, title, track) = DEMO.with(|cell| {
            cell.get_or_init(|| {
                let d = demo::build();
                (
                    d.map.notes,
                    d.map.title,
                    audio::from_mono(&d.samples, demo::SAMPLE_RATE),
                )
            })
            .clone()
        });
        Preview::new(Map::new(title, "", notes, None), Some(track), settings)
    }

    pub fn new(map: Map, track: Option<StaticSoundData>, settings: &Settings) -> Preview {
        let times: Vec<f64> = map.notes.iter().map(|n| n.time).collect();
        let approach = f64::from(settings.approach_distance / settings.approach_rate);
        let mut window = pick_window(&times, approach);
        if let Some(d) = track.as_ref().map(|t| t.duration().as_secs_f64()) {
            // Keep the loop inside the audio when the song is shorter than the chart window.
            if window.1 > d && d > 1.0 {
                window = ((d - WINDOW).max(window.0.min(d - 1.0)), d);
            }
        }
        let music = Music::new(
            track,
            settings.music_volume,
            settings.sfx_volume,
            settings.audio_buffer as u32,
        );
        let game = Game::new(
            map,
            music,
            crate::look::palette(settings.look),
            settings.clone(),
        );
        Preview {
            game,
            fx: PostFx::new(),
            out: None,
            window,
            volumes: (settings.music_volume, settings.sfx_volume),
            volumes_at: 0.0,
            hidden: false,
            began: None,
            cost: (0.0, 0),
            drawn_at: f64::NEG_INFINITY,
        }
    }

    /// Mirrors `settings` (look, space, offset, approach, sizes, volumes) and advances the loop.
    pub fn update(&mut self, settings: &Settings) {
        self.began = Some(std::time::Instant::now());
        let volumes = (settings.music_volume, settings.sfx_volume);
        let now = get_time();
        if volumes != self.volumes && now - self.volumes_at >= VOLUME_EVERY {
            self.game.music.set_volumes(volumes.0, volumes.1);
            self.volumes = volumes;
            self.volumes_at = now;
        }
        self.fx.enabled = settings.glow;
        self.hidden = false;
        self.game.preview(settings, self.window);
    }

    /// Pauses the preview (no room on screen); the next `update` restarts the loop.
    pub fn hide(&mut self) {
        if !self.hidden {
            self.hidden = true;
            self.game.music.set_paused(true);
            self.game.phase = crate::game::Phase::Ready;
        }
    }

    /// Re-renders on the next `draw` whatever the preview frame rate.
    pub fn redraw(&mut self) {
        self.drawn_at = f64::NEG_INFINITY;
    }

    /// Renders the game into `area` (screen pixels) with a thin frame, at most `rate` times a
    /// second (the last picture is shown in between).
    pub fn draw(&mut self, skin: &Skin, area: Rect, rate: Option<f64>) {
        let (w, h) = (
            area.w.round().max(16.0) as u32,
            area.h.round().max(16.0) as u32,
        );
        let stale = self
            .out
            .as_ref()
            .is_none_or(|t| (t.texture.width() as u32, t.texture.height() as u32) != (w, h));
        if stale {
            let t = render_target(w, h);
            t.texture.set_filter(FilterMode::Linear);
            self.out = Some(t);
        }
        let out = self.out.as_ref().expect("target just made");
        let now = get_time();
        if stale || due(now, self.drawn_at, rate) {
            self.game.draw_into(skin, &mut self.fx, out);
            self.drawn_at = now;
        }
        draw_texture_ex(
            &out.texture,
            area.x,
            area.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(area.size()),
                flip_y: true,
                ..Default::default()
            },
        );
        draw_rectangle_lines(
            area.x - 1.0,
            area.y - 1.0,
            area.w + 2.0,
            area.h + 2.0,
            1.0,
            Color::new(1.0, 1.0, 1.0, 0.16),
        );
        // CPU cost of update + draw (command submission, not GPU time), RHYTHM_PERF only.
        if let Some(began) = self.began.take()
            && std::env::var_os("RHYTHM_PERF").is_some()
        {
            self.cost.0 += began.elapsed().as_secs_f64() * 1000.0;
            self.cost.1 += 1;
            if self.cost.1 == 300 {
                eprintln!(
                    "preview: {:.3} ms/frame cpu ({}x{})",
                    self.cost.0 / 300.0,
                    w,
                    h
                );
                self.cost = (0.0, 0);
            }
        }
    }

    /// The preview song's playback time, seconds (drives its music video).
    pub fn song_time(&self) -> f64 {
        self.game.music.time()
    }

    /// The looped stretch, seconds of song time.
    #[allow(dead_code)]
    pub fn window(&self) -> (f64, f64) {
        self.window
    }
}

/// Picks a ~12 s loop that starts a little before the first dense stretch of notes:
/// the first note whose next `DENSE_SPAN` seconds hold at least 3/4 of the densest span.
/// Falls back to the first note minus the approach time.
pub fn pick_window(times: &[f64], approach: f64) -> (f64, f64) {
    let Some(&first) = times.first() else {
        return (0.0, WINDOW);
    };
    let count_from = |i: usize| {
        let end = times[i] + DENSE_SPAN;
        times[i..].partition_point(|t| *t < end)
    };
    let densest = (0..times.len()).map(count_from).max().unwrap_or(0);
    let dense = (0..times.len())
        .find(|&i| densest >= 4 && count_from(i) * 4 >= densest * 3)
        .map_or(first, |i| times[i]);
    let start = (dense - approach - RUN_UP)
        .max(first - approach - RUN_UP)
        .max(-1.0);
    // Never begin later than needed to show the first note when the chart is short.
    let start = start
        .min(times.last().copied().unwrap_or(first) - approach - RUN_UP)
        .max(-1.0);
    (start, start + WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_starts_before_the_first_dense_stretch() {
        // sparse quarter notes for 8 s, then eighths from 8 s on
        let mut times: Vec<f64> = (0..16).map(|i| 0.5 + i as f64 * 0.5).collect();
        times.extend((0..64).map(|i| 8.5 + i as f64 * 0.25));
        let (a, b) = pick_window(&times, 0.5);
        assert!((b - a - WINDOW).abs() < 1e-9);
        // starts in the run-up to the eighths, not at the very first sparse note
        assert!(a < 8.5 - 0.5 && a > 8.5 - 0.5 - 4.0, "{a}");
        assert!(b > 8.5 + 6.0);
    }

    #[test]
    fn window_falls_back_sensibly() {
        assert_eq!(pick_window(&[], 0.5), (0.0, WINDOW));
        let (a, _) = pick_window(&[3.0], 0.5);
        assert!((a - (3.0 - 0.5 - RUN_UP)).abs() < 1e-9);
        let (a, _) = pick_window(&[0.1, 0.2], 1.0);
        assert_eq!(a, -1.0);
    }

    #[test]
    fn preview_rate_limits_redraws() {
        let frames = |rate: Option<f64>, hz: f64| {
            let (mut drawn_at, mut n) = (f64::NEG_INFINITY, 0);
            for i in 0..(hz as usize) {
                let now = i as f64 / hz;
                if due(now, drawn_at, rate) {
                    drawn_at = now;
                    n += 1;
                }
            }
            n
        };
        assert_eq!(frames(None, 144.0), 144);
        assert_eq!(frames(Some(60.0), 60.0), 60);
        assert!(
            (29..=37).contains(&frames(Some(30.0), 144.0)),
            "{}",
            frames(Some(30.0), 144.0)
        );
        assert!((48..=72).contains(&frames(Some(60.0), 144.0)));
    }

    #[test]
    fn the_demo_window_holds_notes() {
        let d = demo::build();
        let times: Vec<f64> = d.map.notes.iter().map(|n| n.time).collect();
        let (a, b) = pick_window(&times, 0.5);
        let inside = times.iter().filter(|t| **t > a && **t < b).count();
        assert!(inside > 20, "{inside} notes in {a}..{b}");
    }
}
