#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod calibrate;
mod chart;
mod demo;
mod difficulty;
mod game;
mod import;
mod look;
mod map;
mod offsets;
mod perf;
mod postfx;
mod preview;
mod priority;
mod render;
#[cfg(test)]
mod robust;
mod scores;
mod settings;
mod sfx;
mod skin;
mod stats;
mod store;
mod video;
mod wallpaper;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use kira::sound::static_sound::StaticSoundData;
use macroquad::prelude::*;

use crate::{
    audio::Music,
    calibrate::Calibration,
    game::{Action, Game},
    map::{Map, Meta},
    offsets::VideoOffsets,
    perf::{Cap, FpsMeter, Limiter, Pacer, Perf},
    render::{Style, Text},
    scores::{Best, Scores},
    settings::{Exit, Settings, SettingsScreen},
    skin::Skin,
};

const MAPS_DIR: &str = "maps";
const ROW_H: f32 = 44.0;
/// Frame rate of every screen but a running song while the window is in the background.
const UNFOCUSED_FPS: f32 = 15.0;

/// RHYTHM_VSYNC=0 / =1 forces vsync off / on whatever the frame-rate setting says.
fn vsync_override() -> Option<bool> {
    std::env::var_os("RHYTHM_VSYNC").map(|v| v != "0")
}

type Stamp = Option<std::time::SystemTime>;
type MetaCache = HashMap<PathBuf, (Stamp, Option<Meta>)>;

static METAS: LazyLock<Mutex<MetaCache>> = LazyLock::new(Mutex::default);

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn request_meta(paths: &[PathBuf]) {
    let todo: Vec<PathBuf> = {
        let mut metas = METAS.lock().unwrap_or_else(|e| e.into_inner());
        let fresh: Vec<PathBuf> = paths
            .iter()
            .filter(|p| metas.get(*p).is_none_or(|(seen, _)| *seen != stamp(p)))
            .cloned()
            .collect();
        for p in &fresh {
            metas.insert(p.clone(), (stamp(p), None));
        }
        fresh
    };
    if todo.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for path in todo {
            let seen = stamp(&path);
            let meta = Map::load(&path).ok().map(|m| m.meta());
            METAS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path, (seen, meta));
        }
    });
}

fn demo_meta() -> Meta {
    static DEMO: std::sync::OnceLock<Meta> = std::sync::OnceLock::new();
    DEMO.get_or_init(|| Meta {
        title: String::new(),
        ..demo::build().map.meta()
    })
    .clone()
}

fn meta_of(path: &Path) -> Option<Meta> {
    METAS.try_lock().ok()?.get(path)?.1.clone()
}

fn bench_from_env() -> Option<(bool, f64)> {
    let value = std::env::var("RHYTHM_BENCH").ok()?;
    let (kind, secs) = value.split_once(':')?;
    Some((kind == "menu", secs.parse().ok()?))
}

fn proc_stat_fields() -> Vec<String> {
    let text = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    text.rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

fn realtime_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|t| std::fs::read_to_string(t.path().join("stat")).ok())
        .filter(|stat| {
            let policy = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(38)?.parse::<u32>().ok());
            matches!(policy, Some(1 | 2))
        })
        .count()
}

fn proc_cpu_seconds() -> f64 {
    let fields = proc_stat_fields();
    let tick = |i: usize| {
        fields
            .get(i)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (tick(11) + tick(12)) / 100.0
}

fn process_age_ms() -> f64 {
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|t| t.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0.0);
    let start = proc_stat_fields()
        .get(19)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
        / 100.0;
    (uptime - start) * 1000.0
}

fn proc_status_mb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|v| {
            v.trim_start_matches(':')
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(0.0)
        / 1024.0
}

fn clock(seconds: f64) -> String {
    let s = seconds.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn window_conf() -> Conf {
    store::use_data_dir();
    let saved = Settings::load();
    if saved.short_gl_queue {
        // SAFETY: runs before the window thread or audio threads exist.
        unsafe { std::env::set_var("__GL_MaxFramesAllowed", "1") };
    }
    Conf {
        window_title: std::env::var("RHYTHM_TITLE").unwrap_or_else(|_| "rhythia is trash".into()),
        window_width: 1280,
        window_height: 720,
        sample_count: 1,
        fullscreen: saved.fullscreen,
        window_resizable: true,
        platform: macroquad::miniquad::conf::Platform {
            // None keeps the platform default (vsync on), as before the frame-rate setting.
            swap_interval: (!vsync_override().unwrap_or(saved.vsync())).then_some(0),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The fps counter: measured rate and the cap in force, top right on every screen.
fn draw_fps(skin: &Skin, fps: f32, cap: Cap, vsync: bool) {
    let text = Text::screen(skin);
    let cap = match cap {
        Cap::Vsync if !vsync => "unlimited".to_string(),
        c => c.label(),
    };
    let label = format!("{fps:.0} fps / {cap}");
    let size = 13.0;
    let w = text.width(&label, size) + 14.0;
    let x = text.screen.x - 8.0;
    draw_rectangle(x - w, 6.0, w, 20.0, Color::new(0.0, 0.0, 0.0, 0.55));
    text.draw(
        &label,
        vec2(x - 7.0, 16.0),
        Style {
            align: render::Align::Right,
            ..Style::new(size, Color::new(1.0, 1.0, 1.0, 0.85))
        }
        .middle(),
    );
}

fn palette() -> Vec<Color> {
    look::active_palette()
}

enum Job {
    Songs(Vec<PathBuf>),
    /// A video wallpaper (Shift+drop): only a copy, but it can be gigabytes.
    Wallpaper(PathBuf),
}

enum Done {
    Songs(import::Report),
    Wallpaper(Result<String, String>),
}

/// Imports run on their own thread so dropping many files one after another never freezes the window.
struct Importer {
    jobs: std::sync::mpsc::Sender<Job>,
    done: std::sync::mpsc::Receiver<Done>,
    pending: usize,
}

impl Importer {
    fn new() -> Self {
        let (jobs, queue) = std::sync::mpsc::channel::<Job>();
        let (finished, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for job in queue {
                let unexpected = "import failed unexpectedly, the file may be damaged";
                let result = match job {
                    Job::Songs(paths) => Done::Songs(
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            import::import(Path::new(MAPS_DIR), &paths)
                        }))
                        .unwrap_or_else(|_| import::Report {
                            errors: vec![unexpected.into()],
                            ..Default::default()
                        }),
                    ),
                    Job::Wallpaper(path) => Done::Wallpaper(
                        std::panic::catch_unwind(|| wallpaper::import(&path))
                            .unwrap_or_else(|_| Err(unexpected.into())),
                    ),
                };
                if finished.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            jobs,
            done,
            pending: 0,
        }
    }

    fn submit(&mut self, job: Job) {
        if self.jobs.send(job).is_ok() {
            self.pending += 1;
        }
    }

    fn poll(&mut self) -> Vec<Done> {
        let done: Vec<Done> = self.done.try_iter().collect();
        self.pending = self.pending.saturating_sub(done.len());
        done
    }
}

#[derive(Clone, PartialEq)]
enum Entry {
    Demo,
    File(PathBuf),
}

impl Entry {
    fn label(&self) -> String {
        match self {
            Entry::Demo => "Demo - 128 BPM (built in)".into(),
            Entry::File(p) => p
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        }
    }

    fn read(&self) -> Result<Song, String> {
        match self {
            Entry::Demo => {
                let d = demo::build();
                Ok((d.map, Some(audio::from_mono(&d.samples, demo::SAMPLE_RATE))))
            }
            Entry::File(path) => {
                let mut map = Map::load(path)?;
                let track = map.audio.take().map(audio::decode).transpose()?;
                Ok((map, track))
            }
        }
    }

    fn load(
        &self,
        settings: &Settings,
        scores: &Scores,
        offsets: &VideoOffsets,
        ready: Option<Song>,
    ) -> Result<Game, String> {
        let started = std::time::Instant::now();
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f32() * 1000.0;
        let mut parse_ms = 0.0;
        let (map, track) = match (ready, self) {
            (Some(song), _) => song,
            (None, Entry::Demo) => {
                let d = demo::build();
                (d.map, Some(audio::from_mono(&d.samples, demo::SAMPLE_RATE)))
            }
            (None, Entry::File(path)) => {
                let mut map = Map::load(path)?;
                parse_ms = ms(started);
                let track = map.audio.take().map(audio::decode).transpose()?;
                if track.is_none() {
                    eprintln!("{}: no audio found, playing silent", path.display());
                }
                (map, track)
            }
        };
        let decode_ms = ms(started) - parse_ms;
        let mut map = map;
        map.notes = difficulty::notes(&map, settings.difficulty);
        let mut music = Music::new(
            track,
            settings.music_volume,
            settings.sfx_volume,
            settings.audio_buffer as u32,
        );
        music.set_rate(difficulty::speed(settings.speed));
        let mut game = Game::new(map, music, palette(), settings.clone());
        game.key = self.label();
        game.tag = difficulty::tag(settings.difficulty, settings.speed);
        game.score_key = difficulty::score_key(&game.key, &game.tag);
        game.best = scores.best(&game.score_key);
        game.load_ms = [parse_ms, decode_ms, ms(started) - parse_ms - decode_ms];
        let clip = game.map.video.as_ref().filter(|_| settings.song_video);
        wallpaper::set_song_video(clip);
        if clip.is_some() {
            game.video_offset_ms = offsets.get(&game.key);
            wallpaper::set_song_video_nudge(game.video_offset_ms / 1000.0);
        }
        Ok(game)
    }
}

type Song = (Map, Option<StaticSoundData>);

const LISTEN_DELAY: f64 = 0.3;
const LISTEN_FADE: f64 = 0.8;
const LISTEN_LENGTH: f64 = 24.0;
const LISTEN_VOLUME: f32 = 0.7;

/// Where the song-select clip starts and how long it loops: from ~40 % in, at most
/// `LISTEN_LENGTH`, never past the end of a short track.
fn listen_window(length: f64) -> (f64, f64) {
    let length = if length.is_finite() {
        length.max(0.0)
    } else {
        0.0
    };
    let from = (length * 0.4).min(length - LISTEN_LENGTH).max(0.0);
    (from, (length - from).clamp(1.0, LISTEN_LENGTH))
}

/// Volume envelope of the clip `t` seconds into a loop of `span` seconds.
fn listen_fade(t: f64, span: f64) -> f32 {
    (t / LISTEN_FADE)
        .min((span - t) / LISTEN_FADE)
        .clamp(0.0, 1.0) as f32
}

/// Request bookkeeping for the song-select loader. Each selection gets a new generation;
/// results of older generations are dropped, and an entry that failed to load is not asked
/// for again until the selection moves to another entry.
struct Loads<E> {
    generation: u64,
    asked: Option<E>,
    failed: Option<E>,
}

impl<E> Default for Loads<E> {
    fn default() -> Self {
        Self {
            generation: 0,
            asked: None,
            failed: None,
        }
    }
}

impl<E: Clone + PartialEq> Loads<E> {
    /// The highlighted entry changed.
    fn select(&mut self, entry: &E) {
        self.cancel();
        if self.failed.as_ref() != Some(entry) {
            self.failed = None;
        }
    }

    /// Whatever is in flight is no longer wanted.
    fn cancel(&mut self) {
        self.generation += 1;
        self.asked = None;
    }

    /// The request to send for `entry`, unless it is already asked for or failed.
    fn ask(&mut self, entry: &E) -> Option<(u64, E)> {
        if self.asked.as_ref() == Some(entry) || self.failed.as_ref() == Some(entry) {
            return None;
        }
        self.asked = Some(entry.clone());
        Some((self.generation, entry.clone()))
    }

    /// A finished load; Some when it answers the current request.
    fn finish<T>(&mut self, generation: u64, result: Result<T, String>) -> Option<T> {
        if generation != self.generation {
            return None;
        }
        let entry = self.asked.take()?;
        result
            .inspect_err(|e| {
                eprintln!("song preview: {e}");
                self.failed = Some(entry);
            })
            .ok()
    }
}

type Loader<J, R> = (
    std::sync::mpsc::Sender<(u64, J)>,
    std::sync::mpsc::Receiver<(u64, R)>,
);

/// One worker thread for `work`: jobs queued while it is busy collapse to the newest, so
/// scrolling through the list decodes at most the song in hand plus the last one asked for.
fn spawn_loader<J: Send + 'static, R: Send + 'static>(
    work: impl Fn(J) -> R + Send + 'static,
) -> Loader<J, R> {
    let (jobs, queue) = std::sync::mpsc::channel::<(u64, J)>();
    let (done, results) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(first) = queue.recv() {
            let (generation, job) = queue.try_iter().last().unwrap_or(first);
            if done.send((generation, work(job))).is_err() {
                break;
            }
        }
    });
    (jobs, results)
}

fn read_entry(entry: Entry) -> Result<Song, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| entry.read()))
        .unwrap_or_else(|_| Err("could not read this song".into()))
}

/// Plays a looping clip of the highlighted song (and its video) in song select, loading it off the main thread.
struct Listen {
    wanted: Option<Entry>,
    since: f64,
    loads: Loads<Entry>,
    jobs: std::sync::mpsc::Sender<(u64, Entry)>,
    results: std::sync::mpsc::Receiver<(u64, Result<Song, String>)>,
    song: Option<(Entry, Song)>,
    music: Option<Music>,
    from: f64,
    span: f64,
    last: f64,
    paused: bool,
}

impl Listen {
    fn new() -> Self {
        let (jobs, results) = spawn_loader(read_entry);
        Self {
            wanted: None,
            since: 0.0,
            loads: Loads::default(),
            jobs,
            results,
            song: None,
            music: None,
            from: 0.0,
            span: LISTEN_LENGTH,
            last: get_time(),
            paused: false,
        }
    }

    fn stop(&mut self) {
        self.music = None;
        self.song = None;
        self.wanted = None;
        self.loads.cancel();
        wallpaper::set_song_video(None);
    }

    fn video_missing(&self) -> bool {
        self.song
            .as_ref()
            .is_some_and(|(_, (map, _))| map.video.is_some())
            && video::available() == Some(false)
    }

    fn take(&mut self, entry: &Entry) -> Option<Song> {
        let song = self.song.take().filter(|(e, _)| e == entry).map(|(_, s)| s);
        self.stop();
        song
    }

    /// Pauses the clip and its video (the window lost focus).
    fn set_paused(&mut self, paused: bool) {
        if paused != self.paused {
            self.paused = paused;
            if let Some(music) = &mut self.music {
                music.set_paused(paused);
            }
        }
    }

    fn update(
        &mut self,
        entry: &Entry,
        settings: &Settings,
        offsets: &VideoOffsets,
        focused: bool,
    ) {
        let now = get_time();
        let dt = now - self.last;
        self.last = now;
        if self.wanted.as_ref() != Some(entry) {
            self.stop();
            self.loads.select(entry);
            self.wanted = Some(entry.clone());
            self.since = now;
        }
        self.set_paused(!focused);
        while let Ok((generation, result)) = self.results.try_recv() {
            if let Some(song) = self.loads.finish(generation, result) {
                self.start(entry.clone(), song, settings, offsets);
            }
        }
        if self.song.is_none()
            && now - self.since > LISTEN_DELAY
            && let Some(job) = self.loads.ask(entry)
        {
            let _ = self.jobs.send(job);
        }
        let Some(music) = &mut self.music else {
            return;
        };
        music.update(dt);
        let t = music.time() - self.from;
        if t > self.span {
            music.restart(self.from);
        }
        let fade = listen_fade(t, self.span);
        music.set_music_volume(settings.music_volume * LISTEN_VOLUME * fade * fade);
        wallpaper::song_video_time(music.time());
    }

    fn start(&mut self, entry: Entry, song: Song, settings: &Settings, offsets: &VideoOffsets) {
        let length = song
            .1
            .as_ref()
            .map_or(song.0.end_time(), |t| t.duration().as_secs_f64());
        (self.from, self.span) = listen_window(length);
        let track = song.1.clone().filter(|_| settings.song_preview);
        let mut music = Music::new(
            track,
            0.0,
            settings.sfx_volume,
            settings.audio_buffer as u32,
        );
        music.restart(self.from);
        music.set_paused(self.paused);
        self.music = Some(music);
        let clip = song.0.video.as_ref().filter(|_| settings.song_video);
        wallpaper::set_song_video(clip);
        wallpaper::set_song_video_nudge(offsets.get(&entry.label()) / 1000.0);
        self.song = Some((entry, song));
    }
}

enum Pick {
    Quit,
    Play(Entry),
    Settings,
    /// Steps of difficulty and of song speed.
    Tune(i32, i32),
}

struct Select {
    entries: Vec<Entry>,
    selected: usize,
    scroll: f32,
    error: Option<String>,
    notice: Option<String>,
    entered: f64,
    highlight: f32,
    last_mouse: Vec2,
    removed: Vec<import::Removed>,
}

impl Select {
    fn new() -> Self {
        let _ = std::fs::create_dir_all(MAPS_DIR);
        let found = map::scan(Path::new(MAPS_DIR));
        request_meta(&found);
        let mut entries = vec![Entry::Demo];
        entries.extend(found.into_iter().map(Entry::File));
        Self {
            entries,
            selected: 0,
            scroll: 0.0,
            error: None,
            notice: None,
            entered: get_time(),
            highlight: 0.0,
            last_mouse: Vec2::from(mouse_position()),
            removed: Vec::new(),
        }
    }
    fn rescan(&mut self, focus: Option<&Path>) {
        let found = map::scan(Path::new(MAPS_DIR));
        request_meta(&found);
        self.entries = vec![Entry::Demo];
        self.entries.extend(found.into_iter().map(Entry::File));
        if let Some(i) = focus.and_then(|f| {
            self.entries
                .iter()
                .position(|e| matches!(e, Entry::File(p) if p == f))
        }) {
            self.selected = i;
        }
        self.selected = self.selected.min(self.entries.len() - 1);
    }

    fn apply(&mut self, report: &import::Report) {
        self.rescan(report.added.last().map(PathBuf::as_path));
        if let Some(error) = report.error() {
            self.error = Some(error);
            self.notice = None;
        } else if let Some(notice) = report.notice() {
            self.error = None;
            self.notice = Some(notice);
        }
    }

    fn remove_selected(&mut self) {
        let Some(Entry::File(path)) = self.entries.get(self.selected).cloned() else {
            self.error = Some("the built-in demo can't be removed".into());
            return;
        };
        match import::remove(Path::new(MAPS_DIR), &path) {
            Ok(gone) => {
                self.notice = Some(format!(
                    "removed {}   Z undo, or drag it back in",
                    gone.label
                ));
                self.error = None;
                self.removed.push(gone);
                self.rescan(None);
                self.selected = self.selected.min(self.entries.len() - 1);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn undo_remove(&mut self) {
        let Some(gone) = self.removed.pop() else {
            return;
        };
        match import::restore(&gone) {
            Ok(path) => {
                self.rescan(Some(&path));
                self.notice = Some(format!("restored {}", gone.label));
                self.error = None;
            }
            Err(e) => {
                self.removed.push(gone);
                self.error = Some(e);
            }
        }
    }

    fn update(&mut self) -> Option<Pick> {
        if is_key_pressed(KeyCode::Delete) {
            self.remove_selected();
        }
        if is_key_pressed(KeyCode::Z) {
            self.undo_remove();
        }
        let n = self.entries.len();
        if is_key_pressed(KeyCode::Escape) {
            return Some(Pick::Quit);
        }
        if is_key_pressed(KeyCode::Tab) {
            return Some(Pick::Settings);
        }
        if is_key_pressed(KeyCode::Down) {
            self.selected = (self.selected + 1) % n;
        }
        if is_key_pressed(KeyCode::Up) {
            self.selected = (self.selected + n - 1) % n;
        }
        let step = |less: &[KeyCode], more: &[KeyCode]| {
            i32::from(more.iter().any(|k| is_key_pressed(*k)))
                - i32::from(less.iter().any(|k| is_key_pressed(*k)))
        };
        let level = step(&[KeyCode::Left], &[KeyCode::Right]);
        let speed = step(
            &[KeyCode::Minus, KeyCode::KpSubtract],
            &[KeyCode::Equal, KeyCode::KpAdd],
        );
        if level != 0 || speed != 0 {
            return Some(Pick::Tune(level, speed));
        }
        let (_, wheel) = mouse_wheel();
        if wheel != 0.0 {
            self.selected =
                (self.selected as i32 - wheel.signum() as i32).clamp(0, n as i32 - 1) as usize;
        }
        let mouse = Vec2::from(mouse_position());
        let right = screen_width() - 48.0;
        if is_mouse_button_pressed(MouseButton::Left)
            && mouse.x >= right - 150.0
            && mouse.x <= right
            && (52.0..=104.0).contains(&mouse.y)
        {
            return Some(Pick::Settings);
        }
        let bottom = screen_height() - 86.0;
        let inside = mouse.x >= 48.0
            && mouse.x <= screen_width() - 48.0
            && mouse.y >= self.top()
            && mouse.y < bottom;
        if inside {
            let row = ((mouse.y - self.top() + self.scroll) / ROW_H).floor() as usize;
            if row < n {
                if mouse != self.last_mouse {
                    self.selected = row;
                }
                if is_mouse_button_pressed(MouseButton::Left) {
                    self.selected = row;
                    self.last_mouse = mouse;
                    return Some(Pick::Play(self.entries[row].clone()));
                }
            }
        }
        self.last_mouse = mouse;
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
            return Some(Pick::Play(self.entries[self.selected].clone()));
        }
        let room = (bottom - self.top()).max(ROW_H);
        let target = (self.selected as f32 * ROW_H - room * 0.45)
            .clamp(0.0, (n as f32 * ROW_H - room).max(0.0));
        let ease = 1.0 - (-get_frame_time() * 16.0).exp();
        self.scroll += (target - self.scroll) * ease;
        self.highlight += (self.selected as f32 - self.highlight) * ease;
        None
    }

    fn top(&self) -> f32 {
        screen_height() * 0.24
    }

    fn draw(
        &self,
        skin: &Skin,
        fx: &mut postfx::PostFx,
        settings: &Settings,
        scores: &Scores,
        status: Option<&str>,
    ) {
        let cam = fx.begin(&render::camera(Vec2::ZERO, settings));
        render::draw_space(
            skin,
            &cam,
            palette()[1],
            settings.space,
            settings.approach_rate,
        );
        gl_use_default_material();
        fx.present();
        let text = Text::screen(skin);
        let w = text.screen.x;
        let intro = (1.0 - (-((get_time() - self.entered) as f32) * 8.0).exp()).clamp(0.0, 1.0);
        let tint = |alpha| Color::new(1.0, 1.0, 1.0, alpha * intro);
        let slide = (1.0 - intro) * 12.0;
        text.draw(
            "rhythia is trash",
            vec2(64.0 + slide, 86.0),
            Style::new(46.0, tint(1.0)).bold(true),
        );
        text.draw(
            "FREE FOREVER   AIM / FLOW / REPEAT",
            vec2(66.0 + slide, 118.0),
            Style::new(13.0, tint(0.45)),
        );
        let count = format!(
            "{} SONG{}",
            self.entries.len(),
            if self.entries.len() == 1 { "" } else { "S" }
        );
        let right_aligned = |size: f32, alpha: f32| Style {
            align: render::Align::Right,
            ..Style::new(size, tint(alpha))
        };
        text.draw(
            "SETTINGS  TAB",
            vec2(w - 48.0, 74.0),
            right_aligned(14.0, 0.7).bold(true),
        );
        text.draw(&count, vec2(w - 48.0, 98.0), right_aligned(13.0, 0.45));
        let level = settings.difficulty.min(difficulty::NAMES.len() - 1);
        let tag = difficulty::tag(settings.difficulty, settings.speed);
        text.draw(
            &format!(
                "{}   {}x",
                difficulty::NAMES[level].to_uppercase(),
                difficulty::speed(settings.speed)
            ),
            vec2(w - 48.0, 128.0),
            right_aligned(14.0, if tag.is_empty() { 0.55 } else { 0.9 }).bold(true),
        );
        let bottom = screen_height() - 86.0;
        let y = self.top() + self.highlight * ROW_H - self.scroll;
        let sy = y.max(self.top());
        let ey = (y + ROW_H - 6.0).min(bottom);
        if ey > sy {
            draw_rectangle(
                48.0,
                sy,
                w - 96.0,
                ey - sy,
                Color::new(1.0, 0.2, 0.45, 0.07 * intro),
            );
            draw_rectangle(48.0, sy, 3.0, ey - sy, Color::new(1.0, 0.1, 0.4, intro));
        }
        for (i, e) in self.entries.iter().enumerate() {
            let y = self.top() + i as f32 * ROW_H - self.scroll;
            if y < self.top() || y + ROW_H > bottom {
                continue;
            }
            let on = i == self.selected;
            let color = tint(if on { 1.0 } else { 0.5 });
            let center = y + (ROW_H - 6.0) / 2.0;
            text.draw(
                &format!("{:02}", i + 1),
                vec2(68.0, center),
                Style::new(12.0, tint(0.3)).middle(),
            );
            let meta = match e {
                Entry::File(p) => meta_of(p),
                Entry::Demo => Some(demo_meta()),
            };
            let name = meta
                .as_ref()
                .map(|m| m.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| e.label());
            if let Some(meta) = &meta {
                let as_mapped = level > difficulty::NORMAL
                    && meta.notes[level] == meta.notes[difficulty::NORMAL]
                    && meta.stars[level] == meta.stars[difficulty::NORMAL];
                let mut detail = format!(
                    "{:.1} stars{}   {} notes   {}",
                    difficulty::stars_at(meta.stars[level], settings.speed),
                    if as_mapped { " (as mapped)" } else { "" },
                    meta.notes[level],
                    clock(meta.length / difficulty::speed(settings.speed))
                );
                if !meta.mappers.is_empty() {
                    detail = format!("{detail}   {}", meta.mappers);
                }
                text.draw(
                    &text.fit(&detail, 13.0, 300.0),
                    vec2(w - 200.0, center),
                    Style {
                        align: render::Align::Right,
                        ..Style::new(13.0, tint(if on { 0.6 } else { 0.3 }))
                    }
                    .middle(),
                );
            } else if let Entry::File(p) = e
                && map::needs_ffmpeg(p)
                && video::available() == Some(false)
            {
                text.draw(
                    "needs ffmpeg (ffmpeg.org)",
                    vec2(w - 200.0, center),
                    Style {
                        align: render::Align::Right,
                        ..Style::new(
                            13.0,
                            Color::new(1.0, 0.45, 0.5, if on { 0.9 } else { 0.5 } * intro),
                        )
                    }
                    .middle(),
                );
            }
            let label = text.fit(&name, 20.0, w - 560.0);
            text.draw(
                &label,
                vec2(104.0 + if on { 4.0 } else { 0.0 }, center),
                Style::new(20.0, color).bold(on).middle(),
            );
            if let Some(Best { accuracy, .. }) =
                scores.best(&difficulty::score_key(&e.label(), &tag))
            {
                text.draw(
                    &format!("{accuracy:.2}%  {}", game::grade_for(accuracy)),
                    vec2(w - 100.0, center),
                    Style {
                        align: render::Align::Right,
                        ..Style::new(14.0, tint(if on { 0.8 } else { 0.4 }))
                    }
                    .middle(),
                );
            }
            if on {
                let x = w - 78.0;
                draw_triangle(
                    vec2(x - 4.0, center - 5.0),
                    vec2(x - 4.0, center + 5.0),
                    vec2(x + 4.0, center),
                    color,
                );
            }
        }
        let footer = format!(
            "Enter play   Left/Right difficulty   -/+ speed   Tab settings   Del remove   Esc quit   F2 glow {}   F3 space {}",
            if fx.enabled { "ON" } else { "OFF" },
            settings.space.name()
        );
        text.draw(
            &footer,
            vec2(64.0, screen_height() - 56.0),
            Style::new(14.0, tint(0.55)),
        );
        let message = status
            .or(self.error.as_deref())
            .or(self.notice.as_deref())
            .unwrap_or("Drag maps (.sspm .phxm .rhm .osz .txt), songs or music videos (.mp3 .ogg .mp4 .webm) here   Shift+drop sets a wallpaper");
        let c = if self.error.is_some() && status.is_none() {
            Color::new(1.0, 0.45, 0.5, intro)
        } else if self.notice.is_some() && status.is_none() {
            Color::new(0.55, 1.0, 0.7, intro)
        } else {
            tint(0.3)
        };
        text.draw(
            &text.fit(message, 13.0, w - 128.0),
            vec2(64.0, screen_height() - 28.0),
            Style::new(13.0, c),
        );
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    priority::raise_main_thread();
    let window_ms = process_age_ms();
    let bench = bench_from_env();
    let skin = Skin::load();
    let mut fx = postfx::PostFx::new();
    let mut settings = Settings::load();
    look::set_active(settings.look);
    let mut perf = Perf::new();
    let mut scores = Scores::load();
    let mut pacer = Pacer::new();
    let mut limiter = Limiter::default();
    let mut offsets = VideoOffsets::load();
    video::start_probe();
    let vsync_env = vsync_override();
    let mut shown_vsync = vsync_env.unwrap_or(settings.vsync());
    let mut cap_now = Cap::Vsync;
    let mut frame_delay_ms = 0.0f32;
    let mut fps_meter = FpsMeter::default();
    let report = std::env::var_os("RHYTHM_PERF").is_some();
    if report {
        use macroquad::miniquad::gl;
        for (name, id) in [
            ("vendor", gl::GL_VENDOR),
            ("renderer", 0x1F01),
            ("version", gl::GL_VERSION),
        ] {
            // SAFETY: the GL context exists once main runs; glGetString returns a static C string.
            let value = unsafe { std::ffi::CStr::from_ptr(gl::glGetString(id).cast()) };
            eprintln!("gl {name}: {}", value.to_string_lossy());
        }
    }
    let mut select = Select::new();
    let mut importer = Importer::new();
    let mut config: Option<SettingsScreen> = None;
    let mut listen = Listen::new();
    let mut calibration: Option<Calibration> = None;
    let mut shown_fullscreen = settings.fullscreen;
    let mut was_playing = false;
    let mut shown_bypass = false;
    let mut game = match std::env::args_os().nth(1) {
        Some(path) => {
            match Entry::File(store::from_launch_dir(PathBuf::from(path)))
                .load(&settings, &scores, &offsets, None)
            {
                Ok(g) => Some(g),
                Err(e) => {
                    select.error = Some(e);
                    None
                }
            }
        }
        None => None,
    };
    let first_frame_ms = process_age_ms();
    let mut bench_run: Option<(std::time::Instant, f64)> = None;
    let mut play_frames = 0u32;
    let booted = std::time::Instant::now();
    let mut play_since: Option<std::time::Instant> = None;
    let mut frames_seen = 0u32;
    loop {
        let frame_ms = perf.tick();
        let fps_now = fps_meter.tick(frame_ms);
        frames_seen += 1;
        if let (Some((_, secs)), Some((began, cpu0))) = (bench, bench_run) {
            let wall = began.elapsed().as_secs_f64();
            if wall >= secs {
                let clock = game
                    .as_ref()
                    .map(|g| g.music.clock_stats())
                    .unwrap_or_default();
                let mut v = perf.json(clock);
                v["cpu_pct"] = ((proc_cpu_seconds() - cpu0) / wall * 100.0).into();
                v["rss_mb"] = proc_status_mb("VmRSS").into();
                v["hwm_mb"] = proc_status_mb("VmHWM").into();
                v["window_ms"] = window_ms.into();
                v["first_frame_ms"] = first_frame_ms.into();
                v["pacer_ms"] = pacer.delay_ms().into();
                v["pacer_misses"] = pacer.misses.into();
                v["cap"] = cap_now.label().into();
                v["vsync"] = shown_vsync.into();
                v["frame_delay_ms"] = frame_delay_ms.into();
                let avg_ms = v["frame_ms"]["avg"].as_f64().unwrap_or(0.0);
                v["fps"] = (1000.0 / avg_ms.max(0.001)).into();
                v["video_options"] = format!("{:?}", settings.video_options()).into();
                v["wall_s"] = wall.into();
                v["nice"] = proc_stat_fields()
                    .get(16)
                    .and_then(|n| n.parse::<i64>().ok())
                    .into();
                v["realtime_threads"] = realtime_threads().into();
                if let Some(g) = &game {
                    v["load_ms"] = serde_json::json!({
                        "parse": g.load_ms[0], "decode": g.load_ms[1], "music": g.load_ms[2],
                    });
                    v["sweep_hits"] = g.run.sweep_hits.into();
                    v["hits"] = g.run.hits.into();
                    v["misses"] = g.run.misses.into();
                }
                println!("BENCH {v}");
                break;
            }
        }
        let playing = config.is_none()
            && calibration.is_none()
            && game
                .as_ref()
                .is_some_and(|g| g.phase == game::Phase::Playing);
        if playing && !was_playing {
            perf.reset();
        }
        if let Some((menu, _)) = bench
            && bench_run.is_none()
        {
            play_frames += u32::from(playing);
            if playing && play_since.is_none() {
                play_since = Some(std::time::Instant::now());
            }
            let settled = |since: std::time::Instant| since.elapsed().as_secs_f64() > 2.5;
            if (menu && frames_seen > 120 && settled(booted))
                || (!menu && play_frames > 180 && play_since.is_some_and(settled))
            {
                perf.reset();
                bench_run = Some((std::time::Instant::now(), proc_cpu_seconds()));
            }
            if !menu
                && let Some(g) = &mut game
                && g.phase == game::Phase::Ready
            {
                g.autostart();
            }
        }
        was_playing = playing;
        // No XGetInputFocus round trips during play: the unfocused cap never applies then
        // (FocusIn/FocusOut still update focus; the poll resumes after the song).
        macroquad::miniquad::window::set_focus_poll(!playing);
        // A benchmark window sits on another monitor, usually without focus.
        let focused = bench.is_some() || macroquad::miniquad::window::is_focused();
        let idle = !focused && !playing;
        let vsync = vsync_env.unwrap_or(settings.vsync());
        if vsync != shown_vsync {
            macroquad::miniquad::window::set_swap_interval(i32::from(vsync));
            shown_vsync = vsync;
        }
        cap_now = if idle {
            Cap::Fps(UNFOCUSED_FPS)
        } else if game.is_some() || calibration.is_some() {
            settings.game_cap()
        } else {
            settings.menu_cap(vsync)
        };
        // The low-latency pacer trims the vsync wait; it has nothing to do under a sleep cap.
        pacer.update(
            playing && settings.low_latency && vsync && cap_now == Cap::Vsync,
            frame_ms,
            perf.stats().p50_ms,
        );
        frame_delay_ms = limiter.delay_ms(cap_now, frame_ms, vsync, pacer.delay_ms());
        macroquad::miniquad::window::set_frame_delay(f64::from(frame_delay_ms) / 1000.0);
        wallpaper::set_paused(idle);
        if is_key_pressed(KeyCode::F3) {
            settings.space = settings.space.next();
        }
        if is_key_pressed(KeyCode::F11) {
            settings.fullscreen = !settings.fullscreen;
        }
        let want_bypass = settings.bypass_compositor && game.is_some() && calibration.is_none();
        if want_bypass != shown_bypass {
            macroquad::miniquad::window::set_bypass_compositor(want_bypass);
            shown_bypass = want_bypass;
        }
        if settings.fullscreen != shown_fullscreen {
            set_fullscreen(settings.fullscreen);
            shown_fullscreen = settings.fullscreen;
        }
        if is_key_pressed(KeyCode::F2) {
            settings.glow = !settings.glow;
        }
        fx.enabled = settings.glow;
        look::set_active(settings.look);
        wallpaper::set_video_look(settings.video_dim, settings.video_options());
        wallpaper::sync(&settings.wallpaper, settings.wallpaper_dim);
        if let Some(c) = &mut calibration {
            if c.update(&mut settings) {
                settings.save();
                calibration = None;
                config = Some(SettingsScreen::new());
                next_frame().await;
                continue;
            }
            c.draw(&skin, &mut fx, &settings);
        } else if let Some(screen) = &mut config {
            match screen.update(&mut settings) {
                Exit::Stay => screen.draw(&skin, &mut fx, &settings),
                Exit::Back => {
                    settings.save();
                    config = None;
                    select.entered = get_time();
                    next_frame().await;
                    continue;
                }
                Exit::Calibrate => {
                    settings.save();
                    config = None;
                    calibration = Some(Calibration::new(&settings));
                    next_frame().await;
                    continue;
                }
            }
        } else if let Some(g) = &mut game {
            let started = std::time::Instant::now();
            let step = g.update();
            if g.video_steps != 0 {
                g.video_offset_ms = offsets.nudge(&g.key, std::mem::take(&mut g.video_steps));
                wallpaper::set_song_video_nudge(g.video_offset_ms / 1000.0);
                offsets.save();
            }
            if g.phase == game::Phase::Finished && !g.recorded {
                g.recorded = true;
                if !g.run.failed {
                    g.new_best = scores.record(
                        &g.score_key,
                        Best {
                            score: g.run.score,
                            accuracy: g.run.accuracy(),
                            max_combo: g.run.max_combo,
                        },
                    );
                    if g.new_best {
                        scores.save();
                    }
                }
                g.best = scores.best(&g.score_key);
            }
            let update_ms = started.elapsed().as_secs_f32() * 1000.0;
            perf.update(update_ms);
            match step {
                Action::Continue => {
                    wallpaper::song_video_time(g.music.time());
                    g.space = settings.space;
                    g.stats = perf.stats();
                    g.stats.delay_ms = frame_delay_ms;
                    g.draw(&skin, &mut fx);
                    perf.cpu(started);
                    let cpu_ms = started.elapsed().as_secs_f32() * 1000.0;
                    if report && (frame_ms > 12.0 || cpu_ms > 4.0) {
                        eprintln!(
                            "spike: frame {frame_ms:.1} ms, cpu {cpu_ms:.1} ms (update {update_ms:.1}, draw {:.1}) at song time {:.2}s",
                            cpu_ms - update_ms,
                            g.now()
                        );
                    }
                }
                Action::Back => {
                    if report {
                        eprintln!(
                            "{} | pacing reached {:.2} ms, cap {}, misses {}, low_latency {}, short_gl_queue {}, bypass {} | sweep-only hits {}",
                            perf.report(g.music.clock_stats()),
                            pacer.reached_ms,
                            pacer
                                .cap_ms()
                                .map_or_else(|| "none".into(), |c| format!("{c:.2} ms")),
                            pacer.misses,
                            settings.low_latency,
                            settings.short_gl_queue,
                            shown_bypass,
                            g.run.sweep_hits
                        );
                    }
                    perf.reset();
                    let out = g.settings_out();
                    settings.sensitivity = out.sensitivity;
                    settings.offset_ms = out.offset_ms;
                    settings.save();
                    wallpaper::set_song_video(None);
                    game = None;
                    select = Select {
                        error: None,
                        ..Select::new()
                    };
                    next_frame().await;
                    continue;
                }
            }
        } else {
            let dropped: Vec<PathBuf> = get_dropped_files()
                .into_iter()
                .filter_map(|f| f.path)
                .collect();
            let as_wallpaper = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
            let (walls, rest): (Vec<PathBuf>, Vec<PathBuf>) = dropped.into_iter().partition(|p| {
                wallpaper::is_image(p) || (as_wallpaper && wallpaper::is_wallpaper(p))
            });
            let mut wallpaper_set =
                |result: Result<String, String>, select: &mut Select| match result {
                    Ok(name) => {
                        settings.wallpaper = name.clone();
                        settings.save();
                        select.notice =
                            Some(format!("wallpaper set to {name} (Tab, Look to change)"));
                        select.error = None;
                    }
                    Err(e) => select.error = Some(e),
                };
            for wall in walls {
                if wallpaper::is_image(&wall) {
                    // decoded into a texture, so it stays on this thread (images are capped at 64 MB)
                    wallpaper_set(wallpaper::import(&wall), &mut select);
                } else {
                    importer.submit(Job::Wallpaper(wall));
                }
            }
            if !rest.is_empty() {
                importer.submit(Job::Songs(rest));
            }
            for done in importer.poll() {
                match done {
                    Done::Songs(report) => select.apply(&report),
                    Done::Wallpaper(result) => wallpaper_set(result, &mut select),
                }
            }
            match select.update() {
                Some(Pick::Quit) => break,
                Some(Pick::Tune(level, speed)) => {
                    let max_level = difficulty::LEVELS.len() as i32 - 1;
                    let max_speed = difficulty::SPEEDS.len() as i32 - 1;
                    settings.difficulty =
                        (settings.difficulty as i32 + level).clamp(0, max_level) as usize;
                    settings.speed = (settings.speed as i32 + speed).clamp(0, max_speed) as usize;
                    settings.save();
                }
                Some(Pick::Settings) => {
                    let entry = select.entries[select.selected].clone();
                    let mut screen = SettingsScreen::new();
                    if let Some((map, track)) = listen.take(&entry) {
                        let nudge = offsets.get(&entry.label()) / 1000.0;
                        let video = map.video.clone().map(|clip| (clip, nudge));
                        screen.set_preview(preview::Preview::new(map, track, &settings), video);
                    }
                    config = Some(screen);
                    next_frame().await;
                    continue;
                }
                Some(Pick::Play(entry)) => {
                    select.draw(&skin, &mut fx, &settings, &scores, Some("loading..."));
                    next_frame().await;
                    let ready = listen.take(&entry);
                    match entry.load(&settings, &scores, &offsets, ready) {
                        Ok(g) => game = Some(g),
                        Err(e) => select.error = Some(e),
                    }
                    next_frame().await;
                    continue;
                }
                None => {
                    if bench.is_none() {
                        listen.update(&select.entries[select.selected], &settings, &offsets, !idle);
                        if settings.song_video && listen.video_missing() && select.error.is_none() {
                            select.notice =
                                Some("this song has a video, add ffmpeg to see it".into());
                        }
                    }
                    let status = (importer.pending > 0).then_some("importing...");
                    select.draw(&skin, &mut fx, &settings, &scores, status);
                }
            }
        }
        if settings.show_fps {
            draw_fps(&skin, fps_now, cap_now, shown_vsync);
        }
        if is_key_pressed(KeyCode::F12) {
            if let Err(e) = std::fs::create_dir_all("screenshots") {
                eprintln!("screenshot: {e}");
            } else {
                let time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                get_screen_data().export_png(&format!("screenshots/{time}.png"));
            }
        }
        let waited = std::time::Instant::now();
        next_frame().await;
        perf.swap(waited.elapsed().as_secs_f32() * 1000.0, frame_delay_ms);
    }
    settings.save();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_window_never_runs_past_the_track() {
        assert_eq!(listen_window(100.0), (40.0, 24.0));
        assert_eq!(listen_window(30.0), (6.0, 24.0));
        // a 10 s track loops all of it instead of 14 s of silence
        assert_eq!(listen_window(10.0), (0.0, 10.0));
        assert_eq!(listen_window(0.0), (0.0, 1.0));
        assert_eq!(listen_window(f64::NAN), (0.0, 1.0));
        // the fade reaches zero at the real loop end
        assert_eq!(listen_fade(10.0, 10.0), 0.0);
        assert_eq!(listen_fade(5.0, 10.0), 1.0);
        assert!(listen_fade(9.6, 10.0) < 0.6);
    }

    #[test]
    fn loads_drop_stale_results_and_remember_failures() {
        let mut l: Loads<&str> = Loads::default();
        l.select(&"a");
        let (ga, ea) = l.ask(&"a").unwrap();
        assert_eq!(ea, "a");
        assert!(l.ask(&"a").is_none(), "one request per selection");
        // the selection moves on before "a" finishes: its result is stale
        l.select(&"b");
        let (gb, _) = l.ask(&"b").unwrap();
        assert_eq!(l.finish(ga, Ok(1)), None);
        assert_eq!(l.finish(gb, Ok(2)), Some(2));
        // a failure is remembered while the selection stays...
        l.select(&"c");
        let (gc, _) = l.ask(&"c").unwrap();
        assert_eq!(l.finish::<i32>(gc, Err("bad file".into())), None);
        assert!(l.ask(&"c").is_none(), "no retry loop");
        l.cancel(); // leaving song select and coming back to the same entry
        l.select(&"c");
        assert!(l.ask(&"c").is_none());
        // ...and forgotten once it moves
        l.select(&"d");
        l.select(&"c");
        let (old, _) = l.ask(&"c").unwrap();
        // a late failure of an old request does not mark the new entry
        l.select(&"e");
        assert_eq!(l.finish::<i32>(old, Err("late".into())), None);
        assert!(l.ask(&"e").is_some());
    }

    #[test]
    fn the_loader_is_one_thread_that_skips_to_the_newest_request() {
        use std::sync::{Arc, Mutex, mpsc};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let log = seen.clone();
        let (jobs, results) = spawn_loader(move |n: u32| {
            log.lock().unwrap().push(n);
            gate.lock().unwrap().recv().unwrap();
            n * 10
        });
        jobs.send((1, 1)).unwrap();
        // while job 1 decodes, the selection scrolls through 2, 3, 4
        while seen.lock().unwrap().is_empty() {
            std::thread::yield_now();
        }
        for n in 2..=4 {
            jobs.send((u64::from(n), n)).unwrap();
        }
        gate_tx.send(()).unwrap();
        gate_tx.send(()).unwrap();
        let got: Vec<(u64, u32)> = (0..2)
            .map(|_| {
                results
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
            })
            .collect();
        assert_eq!(got, [(1, 10), (4, 40)]);
        assert_eq!(*seen.lock().unwrap(), [1, 4], "2 and 3 were never decoded");
    }
}
