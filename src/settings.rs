use std::fs;

use macroquad::prelude::*;
use serde_json::{Map, Value};

use crate::{
    perf::Cap,
    postfx::PostFx,
    preview::Preview,
    render::{self, Align, Space, Style, Text},
    skin::Skin,
};

const FILE: &str = "settings.json";
const REPEAT_DELAY: f32 = 0.35;
const REPEAT_RATE: f32 = 0.04;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    Sensitivity,
    Offset,
    ApproachRate,
    ApproachDistance,
    Parallax,
    Fov,
    NoteSize,
    CursorSize,
    MusicVolume,
    SfxVolume,
    Glow,
    Space,
    Look,
    Wallpaper,
    WallpaperDim,
    AudioBuffer,
    LowLatency,
    Trail,
    Fullscreen,
    ShortQueue,
    NoFail,
    Bypass,
    NoteCore,
    LevelPulse,
    BeatGlow,
    SongVideo,
    SongPreview,
    GameFps,
    MenuFps,
    VideoFps,
    PreviewFps,
    VideoQuality,
    VideoBlur,
    VideoDim,
    ShowFps,
    Difficulty,
    Speed,
}

/// Choices of the list rows, as saved in settings.json (index = the field's value).
pub const GAME_FPS: [&str; 7] = ["unlimited", "vsync", "60", "120", "144", "240", "360"];
pub const MENU_FPS: [&str; 4] = ["30", "60", "72", "game"];
pub const VIDEO_FPS: [&str; 4] = ["24", "30", "60", "source"];
pub const PREVIEW_FPS: [&str; 3] = ["30", "60", "full"];
pub const VIDEO_QUALITY: [&str; 3] = ["540", "720", "1080"];
pub const VIDEO_BLUR: [&str; 3] = ["off", "light", "heavy"];
const GAME_VSYNC: usize = 1;
const MENU_DEFAULT: usize = 2;
/// Menus sleep 11.5 ms before reading input: about 72 fps on a 144 Hz screen (vsync/2).
pub const MENU_DELAY_MS: f32 = 11.5;

fn choices(field: Field) -> Option<&'static [&'static str]> {
    match field {
        Field::GameFps => Some(&GAME_FPS),
        Field::MenuFps => Some(&MENU_FPS),
        Field::VideoFps => Some(&VIDEO_FPS),
        Field::PreviewFps => Some(&PREVIEW_FPS),
        Field::VideoQuality => Some(&VIDEO_QUALITY),
        Field::VideoBlur => Some(&VIDEO_BLUR),
        Field::Difficulty => Some(&crate::difficulty::LEVELS),
        Field::Speed => Some(&crate::difficulty::SPEEDS),
        _ => None,
    }
}

struct Row {
    key: &'static str,
    label: &'static str,
    field: Field,
    min: f32,
    max: f32,
    step: f32,
}

const fn row(
    key: &'static str,
    label: &'static str,
    field: Field,
    min: f32,
    max: f32,
    step: f32,
) -> Row {
    Row {
        key,
        label,
        field,
        min,
        max,
        step,
    }
}

const ROWS: [Row; 37] = [
    row(
        "difficulty",
        "Difficulty (Left/Right in song list)",
        Field::Difficulty,
        0.0,
        3.0,
        1.0,
    ),
    row(
        "speed",
        "Song speed (-/+ in song list)",
        Field::Speed,
        0.0,
        6.0,
        1.0,
    ),
    row(
        "look",
        "Look",
        Field::Look,
        0.0,
        (crate::look::LOOKS.len() - 1) as f32,
        1.0,
    ),
    row(
        "wallpaper_dim",
        "Wallpaper dim",
        Field::WallpaperDim,
        0.0,
        0.85,
        0.01,
    ),
    row("wallpaper", "Wallpaper", Field::Wallpaper, 0.0, 0.0, 1.0),
    row(
        "sensitivity",
        "Sensitivity",
        Field::Sensitivity,
        0.05,
        3.0,
        0.05,
    ),
    row(
        "offset_ms",
        "Audio offset",
        Field::Offset,
        -200.0,
        200.0,
        5.0,
    ),
    row(
        "approach_rate",
        "Note speed",
        Field::ApproachRate,
        10.0,
        60.0,
        1.0,
    ),
    row(
        "approach_distance",
        "Note distance",
        Field::ApproachDistance,
        5.0,
        30.0,
        1.0,
    ),
    row("parallax", "Camera sway", Field::Parallax, 0.0, 1.0, 0.05),
    row("fov", "Field of view", Field::Fov, 50.0, 110.0, 1.0),
    row("note_size", "Note size", Field::NoteSize, 0.5, 1.2, 0.025),
    row(
        "cursor_size",
        "Cursor size",
        Field::CursorSize,
        0.5,
        2.0,
        0.05,
    ),
    row(
        "music_volume",
        "Music volume",
        Field::MusicVolume,
        0.0,
        1.0,
        0.05,
    ),
    row(
        "sfx_volume",
        "Effects volume",
        Field::SfxVolume,
        0.0,
        1.0,
        0.05,
    ),
    row("glow", "Glow", Field::Glow, 0.0, 1.0, 1.0),
    row("space", "Background", Field::Space, 0.0, 2.0, 1.0),
    row(
        "audio_buffer",
        "Audio buffer",
        Field::AudioBuffer,
        0.0,
        1024.0,
        64.0,
    ),
    row(
        "low_latency",
        "Low latency mode",
        Field::LowLatency,
        0.0,
        1.0,
        1.0,
    ),
    row("cursor_trail", "Cursor trail", Field::Trail, 0.0, 1.0, 1.0),
    row(
        "fullscreen",
        "Fullscreen (F11)",
        Field::Fullscreen,
        0.0,
        1.0,
        1.0,
    ),
    row(
        "short_gl_queue",
        "Shorter frame queue (needs restart)",
        Field::ShortQueue,
        0.0,
        1.0,
        1.0,
    ),
    row("no_fail", "No fail", Field::NoFail, 0.0, 1.0, 1.0),
    row("note_core", "Note style", Field::NoteCore, 0.0, 1.0, 1.0),
    row(
        "level_pulse",
        "Level-up pulse",
        Field::LevelPulse,
        0.0,
        1.0,
        1.0,
    ),
    row(
        "beat_glow",
        "Beat background",
        Field::BeatGlow,
        0.0,
        1.0,
        1.0,
    ),
    row("song_video", "Song video", Field::SongVideo, 0.0, 1.0, 1.0),
    row(
        "song_preview",
        "Song preview in song select",
        Field::SongPreview,
        0.0,
        1.0,
        1.0,
    ),
    row(
        "bypass_compositor",
        "Skip desktop compositor while playing",
        Field::Bypass,
        0.0,
        1.0,
        1.0,
    ),
    row(
        "game_fps",
        "Frame rate in game",
        Field::GameFps,
        0.0,
        6.0,
        1.0,
    ),
    row(
        "menu_fps",
        "Frame rate in menus",
        Field::MenuFps,
        0.0,
        3.0,
        1.0,
    ),
    row(
        "video_fps",
        "Music video frame rate",
        Field::VideoFps,
        0.0,
        3.0,
        1.0,
    ),
    row(
        "preview_fps",
        "Settings preview frame rate",
        Field::PreviewFps,
        0.0,
        2.0,
        1.0,
    ),
    row(
        "video_quality",
        "Music video quality",
        Field::VideoQuality,
        0.0,
        2.0,
        1.0,
    ),
    row(
        "video_blur",
        "Music video blur",
        Field::VideoBlur,
        0.0,
        2.0,
        1.0,
    ),
    row(
        "video_dim",
        "Music video dim",
        Field::VideoDim,
        0.0,
        0.9,
        0.01,
    ),
    row("show_fps", "Show fps", Field::ShowFps, 0.0, 1.0, 1.0),
];

fn row_of(field: Field) -> &'static Row {
    ROWS.iter()
        .find(|r| r.field == field)
        .expect("every field has a row")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub sensitivity: f32,
    pub offset_ms: f32,
    pub approach_rate: f32,
    pub approach_distance: f32,
    pub parallax: f32,
    pub fov: f32,
    pub note_size: f32,
    pub cursor_size: f32,
    pub music_volume: f32,
    pub sfx_volume: f32,
    pub glow: bool,
    pub space: Space,
    pub audio_buffer: f32,
    pub low_latency: bool,
    pub cursor_trail: bool,
    pub fullscreen: bool,
    pub short_gl_queue: bool,
    pub no_fail: bool,
    pub note_core: bool,
    pub level_pulse: bool,
    pub beat_glow: bool,
    pub bypass_compositor: bool,
    pub look: usize,
    pub wallpaper: String,
    pub wallpaper_dim: f32,
    pub song_video: bool,
    pub song_preview: bool,
    /// Index into `GAME_FPS`.
    pub game_fps: usize,
    pub difficulty: usize,
    pub speed: usize,
    /// Index into `MENU_FPS`.
    pub menu_fps: usize,
    /// Index into `VIDEO_FPS`.
    pub video_fps: usize,
    /// Index into `PREVIEW_FPS`.
    pub preview_fps: usize,
    /// Index into `VIDEO_QUALITY`.
    pub video_quality: usize,
    /// Index into `VIDEO_BLUR`.
    pub video_blur: usize,
    pub video_dim: f32,
    pub show_fps: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            offset_ms: 0.0,
            approach_rate: 30.0,
            approach_distance: 15.0,
            parallax: 0.25,
            fov: 70.0,
            note_size: 0.875,
            cursor_size: 1.0,
            music_volume: 1.0,
            sfx_volume: 1.0,
            glow: true,
            space: Space::Grid,
            audio_buffer: 256.0,
            low_latency: true,
            cursor_trail: false,
            fullscreen: false,
            short_gl_queue: false,
            no_fail: false,
            note_core: false,
            level_pulse: true,
            beat_glow: false,
            bypass_compositor: true,
            look: 0,
            wallpaper: String::new(),
            wallpaper_dim: 0.55,
            song_video: true,
            song_preview: true,
            game_fps: GAME_VSYNC,
            difficulty: crate::difficulty::NORMAL,
            speed: crate::difficulty::ONE_X,
            menu_fps: MENU_DEFAULT,
            video_fps: 1,
            preview_fps: 1,
            video_quality: 0,
            video_blur: 0,
            video_dim: 0.55,
            show_fps: false,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Ok(text) = fs::read_to_string(FILE) else {
            return Self::default();
        };
        Self::parse(&text).unwrap_or_else(|| {
            eprintln!("{FILE}: not valid settings, using defaults");
            Self::default()
        })
    }

    fn parse(text: &str) -> Option<Self> {
        let mut s = Self::default();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            return None;
        };
        for r in &ROWS {
            let Some(v) = map.get(r.key) else { continue };
            match r.field {
                Field::Glow => s.glow = v.as_bool().unwrap_or(s.glow),
                Field::LowLatency => s.low_latency = v.as_bool().unwrap_or(s.low_latency),
                Field::Trail => s.cursor_trail = v.as_bool().unwrap_or(s.cursor_trail),
                Field::Fullscreen => s.fullscreen = v.as_bool().unwrap_or(s.fullscreen),
                Field::ShortQueue => s.short_gl_queue = v.as_bool().unwrap_or(s.short_gl_queue),
                Field::NoFail => s.no_fail = v.as_bool().unwrap_or(s.no_fail),
                Field::NoteCore => s.note_core = v.as_bool().unwrap_or(s.note_core),
                Field::LevelPulse => s.level_pulse = v.as_bool().unwrap_or(s.level_pulse),
                Field::BeatGlow => s.beat_glow = v.as_bool().unwrap_or(s.beat_glow),
                Field::Bypass => s.bypass_compositor = v.as_bool().unwrap_or(s.bypass_compositor),
                Field::SongVideo => s.song_video = v.as_bool().unwrap_or(s.song_video),
                Field::SongPreview => s.song_preview = v.as_bool().unwrap_or(s.song_preview),
                Field::ShowFps => s.show_fps = v.as_bool().unwrap_or(s.show_fps),
                f if choices(f).is_some() => {
                    // "144", 144 and 144.0 all name the same choice; unknown values keep the default.
                    let name = v
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| v.as_f64().map(|n| n.to_string()));
                    if let Some(i) = name.and_then(|n| {
                        choices(f)
                            .unwrap_or_default()
                            .iter()
                            .position(|c| c.eq_ignore_ascii_case(n.trim()))
                    }) {
                        s.set(f, i as f32);
                    }
                }
                Field::Space => {
                    if let Some(found) = v
                        .as_str()
                        .and_then(|n| Space::ALL.into_iter().find(|sp| sp.name() == n))
                    {
                        s.space = found;
                    }
                }
                f => {
                    if let Some(x) = v.as_f64() {
                        s.set(f, x as f32);
                    }
                }
            }
        }
        s.look = map.get("look").and_then(Value::as_u64).unwrap_or(0) as usize;
        s.look = s.look.min(crate::look::LOOKS.len() - 1);
        s.wallpaper = map
            .get("wallpaper")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        s.wallpaper_dim = map
            .get("wallpaper_dim")
            .and_then(Value::as_f64)
            .unwrap_or(0.55) as f32;
        s.wallpaper_dim = s.wallpaper_dim.clamp(0.0, 0.85);
        Some(s)
    }

    pub fn save(&self) {
        if let Err(e) = crate::store::write_atomic(FILE, &self.to_text()) {
            eprintln!("{FILE}: {e}");
        }
    }

    fn to_text(&self) -> String {
        let mut map = Map::new();
        for r in &ROWS {
            let v = match r.field {
                Field::Glow => Value::Bool(self.glow),
                Field::LowLatency => Value::Bool(self.low_latency),
                Field::Trail => Value::Bool(self.cursor_trail),
                Field::Fullscreen => Value::Bool(self.fullscreen),
                Field::ShortQueue => Value::Bool(self.short_gl_queue),
                Field::NoFail => Value::Bool(self.no_fail),
                Field::NoteCore => Value::Bool(self.note_core),
                Field::LevelPulse => Value::Bool(self.level_pulse),
                Field::BeatGlow => Value::Bool(self.beat_glow),
                Field::Bypass => Value::Bool(self.bypass_compositor),
                Field::SongVideo => Value::Bool(self.song_video),
                Field::SongPreview => Value::Bool(self.song_preview),
                Field::ShowFps => Value::Bool(self.show_fps),
                Field::Space => Value::String(self.space.name().into()),
                f if choices(f).is_some() => {
                    let list = choices(f).unwrap_or_default();
                    Value::String(list[(self.get(f) as usize).min(list.len() - 1)].into())
                }
                f => Value::from((f64::from(self.get(f)) * 1000.0).round() / 1000.0),
            };
            map.insert(r.key.into(), v);
        }
        map.insert(
            "look".into(),
            Value::from(self.look.min(crate::look::LOOKS.len() - 1)),
        );
        map.insert("wallpaper".into(), Value::String(self.wallpaper.clone()));
        map.insert(
            "wallpaper_dim".into(),
            Value::from((f64::from(self.wallpaper_dim.clamp(0.0, 0.85)) * 1000.0).round() / 1000.0),
        );
        serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default()
    }

    fn reset_field(&mut self, field: Field) {
        let d = Settings::default();
        match field {
            Field::Wallpaper => self.wallpaper = d.wallpaper,
            Field::Look => {
                self.look = d.look;
                self.space = crate::look::get(d.look).space;
            }
            f => self.set(f, d.get(f)),
        }
    }

    fn reset_tab(&mut self, tab: usize) {
        for f in TABS[tab].main.iter().chain(TABS[tab].more.iter()) {
            self.reset_field(*f);
        }
    }

    fn differs(&self, field: Field) -> bool {
        let d = Settings::default();
        match field {
            Field::Wallpaper => self.wallpaper != d.wallpaper,
            f => (self.get(f) - d.get(f)).abs() > 1e-4,
        }
    }

    pub fn adjust_offset(&mut self, delta_ms: f32) {
        self.set(Field::Offset, self.offset_ms + delta_ms);
    }

    pub fn zero_offset(&mut self) {
        self.set(Field::Offset, 0.0);
    }

    /// Whether the saved setting wants vsync (RHYTHM_VSYNC can still override it).
    pub fn vsync(&self) -> bool {
        self.game_fps == GAME_VSYNC
    }

    /// The frame-rate cap while a song (or calibration) is on screen.
    pub fn game_cap(&self) -> Cap {
        match GAME_FPS.get(self.game_fps).copied() {
            Some("unlimited") => Cap::Unlimited,
            Some("vsync") | None => Cap::Vsync,
            Some(n) => Cap::Fps(n.parse().unwrap_or(60.0)),
        }
    }

    /// The frame-rate cap in menus. `vsync`: whether vsync is really on.
    pub fn menu_cap(&self, vsync: bool) -> Cap {
        match MENU_FPS.get(self.menu_fps).copied() {
            Some("game") => self.game_cap(),
            // the old fixed menu sleep is half the refresh rate only with vsync on
            Some("72") | None if vsync => Cap::Delay(MENU_DELAY_MS),
            Some("72") | None => Cap::Fps(72.0),
            Some(n) => Cap::Fps(n.parse().unwrap_or(60.0)),
        }
    }

    /// Decode settings for music videos (and, without blur, video wallpapers).
    pub fn video_options(&self) -> crate::video::Options {
        crate::video::Options {
            height: VIDEO_QUALITY
                .get(self.video_quality)
                .and_then(|q| q.parse().ok())
                .unwrap_or(540),
            fps: VIDEO_FPS.get(self.video_fps).and_then(|f| f.parse().ok()),
            blur: self.video_blur.min(2) as u8,
        }
    }

    /// How often the settings preview redraws; None = every frame.
    pub fn preview_rate(&self) -> Option<f64> {
        PREVIEW_FPS
            .get(self.preview_fps)
            .and_then(|f| f.parse().ok())
    }

    /// Battery saver: menus at 30 fps, videos at 24, preview at 30, no glow.
    pub fn battery_saver(&mut self) {
        self.menu_fps = 0;
        self.video_fps = 0;
        self.preview_fps = 0;
        self.glow = false;
    }

    pub fn is_battery_saver(&self) -> bool {
        let mut saver = self.clone();
        saver.battery_saver();
        saver == *self
    }

    fn get(&self, field: Field) -> f32 {
        match field {
            Field::Sensitivity => self.sensitivity,
            Field::Offset => self.offset_ms,
            Field::ApproachRate => self.approach_rate,
            Field::ApproachDistance => self.approach_distance,
            Field::Parallax => self.parallax,
            Field::Fov => self.fov,
            Field::NoteSize => self.note_size,
            Field::CursorSize => self.cursor_size,
            Field::MusicVolume => self.music_volume,
            Field::SfxVolume => self.sfx_volume,
            Field::Glow => f32::from(u8::from(self.glow)),
            Field::AudioBuffer => self.audio_buffer,
            Field::LowLatency => f32::from(u8::from(self.low_latency)),
            Field::Trail => f32::from(u8::from(self.cursor_trail)),
            Field::Fullscreen => f32::from(u8::from(self.fullscreen)),
            Field::ShortQueue => f32::from(u8::from(self.short_gl_queue)),
            Field::NoFail => f32::from(u8::from(self.no_fail)),
            Field::NoteCore => f32::from(u8::from(self.note_core)),
            Field::LevelPulse => f32::from(u8::from(self.level_pulse)),
            Field::BeatGlow => f32::from(u8::from(self.beat_glow)),
            Field::Bypass => f32::from(u8::from(self.bypass_compositor)),
            Field::SongVideo => f32::from(u8::from(self.song_video)),
            Field::SongPreview => f32::from(u8::from(self.song_preview)),
            Field::ShowFps => f32::from(u8::from(self.show_fps)),
            Field::GameFps => self.game_fps as f32,
            Field::Difficulty => self.difficulty as f32,
            Field::Speed => self.speed as f32,
            Field::MenuFps => self.menu_fps as f32,
            Field::VideoFps => self.video_fps as f32,
            Field::PreviewFps => self.preview_fps as f32,
            Field::VideoQuality => self.video_quality as f32,
            Field::VideoBlur => self.video_blur as f32,
            Field::VideoDim => self.video_dim,
            Field::WallpaperDim => self.wallpaper_dim,
            Field::Look => self.look as f32,
            Field::Wallpaper => 0.0,
            Field::Space => Space::ALL
                .iter()
                .position(|s| *s == self.space)
                .unwrap_or(0) as f32,
        }
    }

    fn set(&mut self, field: Field, value: f32) {
        let r = row_of(field);
        let v = value.clamp(r.min, r.max);
        match field {
            Field::Sensitivity => self.sensitivity = v,
            Field::Offset => self.offset_ms = v,
            Field::ApproachRate => self.approach_rate = v,
            Field::ApproachDistance => self.approach_distance = v,
            Field::Parallax => self.parallax = v,
            Field::Fov => self.fov = v,
            Field::NoteSize => self.note_size = v,
            Field::CursorSize => self.cursor_size = v,
            Field::MusicVolume => self.music_volume = v,
            Field::SfxVolume => self.sfx_volume = v,
            Field::Glow => self.glow = v >= 0.5,
            Field::AudioBuffer => self.audio_buffer = v,
            Field::LowLatency => self.low_latency = v >= 0.5,
            Field::Trail => self.cursor_trail = v >= 0.5,
            Field::Fullscreen => self.fullscreen = v >= 0.5,
            Field::ShortQueue => self.short_gl_queue = v >= 0.5,
            Field::NoFail => self.no_fail = v >= 0.5,
            Field::NoteCore => self.note_core = v >= 0.5,
            Field::LevelPulse => self.level_pulse = v >= 0.5,
            Field::BeatGlow => self.beat_glow = v >= 0.5,
            Field::Bypass => self.bypass_compositor = v >= 0.5,
            Field::SongVideo => self.song_video = v >= 0.5,
            Field::SongPreview => self.song_preview = v >= 0.5,
            Field::ShowFps => self.show_fps = v >= 0.5,
            Field::GameFps => self.game_fps = v.round() as usize,
            Field::Difficulty => self.difficulty = v.round() as usize,
            Field::Speed => self.speed = v.round() as usize,
            Field::MenuFps => self.menu_fps = v.round() as usize,
            Field::VideoFps => self.video_fps = v.round() as usize,
            Field::PreviewFps => self.preview_fps = v.round() as usize,
            Field::VideoQuality => self.video_quality = v.round() as usize,
            Field::VideoBlur => self.video_blur = v.round() as usize,
            Field::VideoDim => self.video_dim = v,
            Field::Space => self.space = Space::ALL[v.round() as usize],
            Field::Look => {
                self.look = v.round() as usize;
                self.space = crate::look::get(self.look).space;
            }
            Field::Wallpaper => {}
            Field::WallpaperDim => self.wallpaper_dim = v.clamp(0.0, 0.85),
        }
    }

    fn nudge(&mut self, r: &Row, dir: f32, multiplier: f32) {
        match r.field {
            Field::Glow => self.glow = !self.glow,
            Field::LowLatency => self.low_latency = !self.low_latency,
            Field::Trail => self.cursor_trail = !self.cursor_trail,
            Field::Fullscreen => self.fullscreen = !self.fullscreen,
            Field::ShortQueue => self.short_gl_queue = !self.short_gl_queue,
            Field::NoFail => self.no_fail = !self.no_fail,
            Field::NoteCore => self.note_core = !self.note_core,
            Field::LevelPulse => self.level_pulse = !self.level_pulse,
            Field::BeatGlow => self.beat_glow = !self.beat_glow,
            Field::Bypass => self.bypass_compositor = !self.bypass_compositor,
            Field::SongVideo => self.song_video = !self.song_video,
            Field::SongPreview => self.song_preview = !self.song_preview,
            Field::ShowFps => self.show_fps = !self.show_fps,
            f if choices(f).is_some() => {
                let n = choices(f).unwrap_or_default().len() as i32;
                let i = (self.get(f) as i32 + dir as i32).rem_euclid(n);
                self.set(f, i as f32);
            }
            Field::Look => {
                self.look = (self.look as i32 + dir as i32)
                    .rem_euclid(crate::look::LOOKS.len() as i32)
                    as usize;
                self.space = crate::look::get(self.look).space;
            }
            Field::Wallpaper => {
                let files = crate::wallpaper::scan(std::path::Path::new("wallpapers"));
                let names: Vec<String> = files
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                self.wallpaper = next_wallpaper(&self.wallpaper, &names, dir as i32);
            }
            Field::Space => {
                let n = Space::ALL.len() as i32;
                let i = (self.get(Field::Space) as i32 + dir as i32).rem_euclid(n);
                self.space = Space::ALL[i as usize];
            }
            f => self.set(f, self.get(f) + dir * r.step * multiplier),
        }
    }

    fn show(&self, field: Field) -> String {
        match field {
            Field::Sensitivity => format!("{:.2}", self.sensitivity),
            Field::Offset => format!("{:+.0} ms", self.offset_ms),
            Field::ApproachRate => format!("{:.0}", self.approach_rate),
            Field::ApproachDistance => format!("{:.0}", self.approach_distance),
            Field::Parallax => format!("{:.2}", self.parallax),
            Field::Fov => format!("{:.0} deg", self.fov),
            Field::NoteSize => format!("{:.3}", self.note_size),
            Field::CursorSize => format!("{:.2}x", self.cursor_size),
            Field::MusicVolume => format!("{:.0}%", self.music_volume * 100.0),
            Field::SfxVolume => format!("{:.0}%", self.sfx_volume * 100.0),
            Field::Glow => (if self.glow { "on" } else { "off" }).to_string(),
            Field::LowLatency => (if self.low_latency { "on" } else { "off" }).to_string(),
            Field::Trail => (if self.cursor_trail { "on" } else { "off" }).to_string(),
            Field::Fullscreen => (if self.fullscreen { "on" } else { "off" }).to_string(),
            Field::ShortQueue => (if self.short_gl_queue { "on" } else { "off" }).to_string(),
            Field::NoFail => (if self.no_fail { "on" } else { "off" }).to_string(),
            Field::NoteCore => (if self.note_core { "filled" } else { "outline" }).to_string(),
            Field::LevelPulse => (if self.level_pulse { "on" } else { "off" }).to_string(),
            Field::BeatGlow => (if self.beat_glow { "on" } else { "off" }).to_string(),
            Field::Bypass => (if self.bypass_compositor { "on" } else { "off" }).to_string(),
            Field::SongVideo => (if self.song_video { "on" } else { "off" }).to_string(),
            Field::SongPreview => (if self.song_preview { "on" } else { "off" }).to_string(),
            Field::ShowFps => (if self.show_fps { "on" } else { "off" }).to_string(),
            Field::GameFps => match GAME_FPS[self.game_fps.min(GAME_FPS.len() - 1)] {
                "vsync" => "vsync (default)".to_string(),
                "unlimited" => "unlimited".to_string(),
                n => format!("{n} fps"),
            },
            Field::MenuFps => match MENU_FPS[self.menu_fps.min(MENU_FPS.len() - 1)] {
                "game" => "same as game".to_string(),
                // an 11.5 ms sleep on top of vsync: 72 fps at 144 Hz, more on faster screens
                "72" if self.vsync() => "~72 fps (default)".to_string(),
                "72" => "72 fps (default)".to_string(),
                n => format!("{n} fps"),
            },
            Field::VideoFps => match VIDEO_FPS[self.video_fps.min(VIDEO_FPS.len() - 1)] {
                "source" => "as recorded".to_string(),
                n => format!("{n} fps"),
            },
            Field::PreviewFps => match PREVIEW_FPS[self.preview_fps.min(PREVIEW_FPS.len() - 1)] {
                "full" => "every frame".to_string(),
                n => format!("{n} fps"),
            },
            Field::VideoQuality => {
                format!(
                    "{}p",
                    VIDEO_QUALITY[self.video_quality.min(VIDEO_QUALITY.len() - 1)]
                )
            }
            Field::VideoBlur => VIDEO_BLUR[self.video_blur.min(VIDEO_BLUR.len() - 1)].to_string(),
            Field::Difficulty => crate::difficulty::NAMES
                [self.difficulty.min(crate::difficulty::NAMES.len() - 1)]
            .to_string(),
            Field::Speed => format!("{}x", crate::difficulty::speed(self.speed)),
            Field::VideoDim => format!("{:.0}%", self.video_dim * 100.0),
            Field::AudioBuffer if self.audio_buffer < 1.0 => "auto".to_string(),
            Field::AudioBuffer => format!(
                "{:.0} ({:.1} ms)",
                self.audio_buffer,
                self.audio_buffer / 48.0
            ),
            Field::Space => self.space.name().to_string(),
            Field::Look => crate::look::get(self.look).name.to_string(),
            Field::Wallpaper if !self.wallpaper.is_empty() => self.wallpaper.clone(),
            Field::Wallpaper => {
                if crate::wallpaper::scan(std::path::Path::new("wallpapers")).is_empty() {
                    "None (drop an image on the song list)".to_string()
                } else {
                    "None".to_string()
                }
            }
            Field::WallpaperDim => format!("{:.0}%", self.wallpaper_dim * 100.0),
        }
    }
}

fn is_choice(field: Field) -> bool {
    matches!(
        field,
        Field::Glow
            | Field::Look
            | Field::Wallpaper
            | Field::Space
            | Field::LowLatency
            | Field::Trail
            | Field::Fullscreen
            | Field::ShortQueue
            | Field::NoFail
            | Field::NoteCore
            | Field::LevelPulse
            | Field::BeatGlow
            | Field::Bypass
            | Field::SongVideo
            | Field::SongPreview
            | Field::ShowFps
            | Field::GameFps
            | Field::MenuFps
            | Field::VideoFps
            | Field::PreviewFps
            | Field::VideoQuality
            | Field::VideoBlur
            | Field::Difficulty
            | Field::Speed
    )
}

const TAB_NAMES: [&str; 4] = ["Look", "Play", "Sound", "Video"];

struct TabRows {
    main: &'static [Field],
    more: &'static [Field],
}

const TABS: [TabRows; 4] = [
    TabRows {
        main: &[
            Field::Look,
            Field::Space,
            Field::Wallpaper,
            Field::WallpaperDim,
            Field::SongVideo,
            Field::VideoDim,
            Field::Trail,
            Field::NoteCore,
        ],
        more: &[Field::Glow, Field::LevelPulse, Field::BeatGlow],
    },
    TabRows {
        main: &[
            Field::Difficulty,
            Field::Speed,
            Field::Sensitivity,
            Field::ApproachRate,
            Field::ApproachDistance,
            Field::NoteSize,
            Field::CursorSize,
        ],
        more: &[Field::Parallax, Field::Fov, Field::NoFail],
    },
    TabRows {
        main: &[
            Field::MusicVolume,
            Field::SfxVolume,
            Field::Offset,
            Field::SongPreview,
        ],
        more: &[Field::AudioBuffer, Field::LowLatency],
    },
    TabRows {
        main: &[
            Field::Fullscreen,
            Field::GameFps,
            Field::MenuFps,
            Field::ShowFps,
        ],
        more: &[
            Field::VideoQuality,
            Field::VideoFps,
            Field::VideoBlur,
            Field::PreviewFps,
            Field::Bypass,
            Field::ShortQueue,
        ],
    },
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Entry {
    Field(Field),
    Calibrate,
    BatterySaver,
    More,
}

fn entries(tab: usize, more: bool) -> Vec<Entry> {
    let t = &TABS[tab];
    let mut out: Vec<Entry> = t.main.iter().map(|f| Entry::Field(*f)).collect();
    if tab == 2 {
        out.push(Entry::Calibrate);
    }
    if tab == 3 {
        out.push(Entry::BatterySaver);
    }
    out.push(Entry::More);
    if more {
        out.extend(t.more.iter().map(|f| Entry::Field(*f)));
    }
    out
}

fn next_wallpaper(current: &str, names: &[String], dir: i32) -> String {
    let mut all: Vec<&str> = vec![""];
    all.extend(names.iter().map(String::as_str));
    let at = all.iter().position(|n| *n == current).unwrap_or(0) as i32;
    all[(at + dir).rem_euclid(all.len() as i32) as usize].to_owned()
}

struct Layout {
    top: f32,
    row_h: f32,
    label_x: f32,
    track0: f32,
    track1: f32,
    value_x: f32,
    tabs_y: f32,
    scale: f32,
    /// Right edge of the rows (highlight and mouse hit area).
    right: f32,
    /// Where the live preview goes, if there is room for it.
    preview: Option<Rect>,
}

const PREVIEW_MIN_H: f32 = 110.0;

fn layout_for(w: f32, h: f32, count: usize) -> Layout {
    let scale = (h / 720.0).clamp(0.7, 1.5);
    let top = 196.0 * scale;
    let bottom = h - 86.0;
    let tabs_y = 156.0 * scale;
    if w >= 1000.0 && w >= h * 1.4 {
        // wide: rows on the left, preview on the right, top-aligned with the rows
        let right = (w * 0.56).round();
        let rw = right - 48.0;
        let track0 = 48.0 + rw * 0.40;
        let px = right + 32.0;
        let pw = w - 48.0 - px;
        let ph = (pw * 9.0 / 16.0).min(bottom - top);
        return Layout {
            top,
            row_h: ((bottom - top) / count.max(1) as f32).clamp(18.0, 38.0 * scale),
            label_x: 104.0,
            track0,
            track1: track0 + rw * 0.30,
            value_x: right - 48.0,
            tabs_y,
            scale,
            right,
            preview: Some(Rect::new(px, top, pw, ph)),
        };
    }
    // narrow: rows across the screen, preview in what is left below them
    let track0 = w * 0.42;
    let row_h = ((bottom - top) / count.max(1) as f32).clamp(18.0, 30.0 * scale);
    let below = top + count as f32 * row_h + 18.0;
    let ph = (h - 76.0 - below).min((w - 96.0) * 9.0 / 16.0);
    Layout {
        top,
        row_h,
        label_x: 104.0,
        track0,
        track1: track0 + (w * 0.28).max(200.0),
        value_x: w - 96.0,
        tabs_y,
        scale,
        right: w - 48.0,
        preview: (ph >= PREVIEW_MIN_H).then(|| Rect::new(48.0, below, ph * 16.0 / 9.0, ph)),
    }
}

fn layout(count: usize) -> Layout {
    layout_for(screen_width(), screen_height(), count)
}

/// Title baseline, title size and subtitle baseline. The header shrinks with the tabs on
/// short windows, or the tabs run into it.
fn header(scale: f32) -> (f32, f32, f32) {
    let hs = scale.min(1.0);
    (86.0 * hs, 46.0 * hs, 118.0 * hs - (1.0 - hs) * 12.0)
}

fn tab_span(i: usize, scale: f32) -> (f32, f32) {
    (64.0 + i as f32 * 118.0 * scale, 100.0 * scale)
}

pub enum Exit {
    Stay,
    Back,
    Calibrate,
}

pub struct SettingsScreen {
    tab: usize,
    more: [bool; 4],
    selected: usize,
    entered: f64,
    last_mouse: Vec2,
    dragging: bool,
    drag_field: Option<Field>,
    held: f32,
    snapshot: Option<Settings>,
    preview: Option<Preview>,
    before: bool,
    drawn_before: bool,
    /// The preview song's video and its per-song nudge (seconds), shown behind the preview
    /// while the song video setting is on (`video_on`).
    video: Option<(crate::video::Clip, f64)>,
    video_on: bool,
}

impl Drop for SettingsScreen {
    /// Leaving settings drops the kept "before" wallpaper and the preview's song video.
    fn drop(&mut self) {
        crate::wallpaper::keep_before(None);
        if self.video_on {
            crate::wallpaper::set_song_video(None);
        }
    }
}

/// What the preview's backdrop shows instead of the live wallpaper: the shown settings'
/// wallpaper and dims, but only while comparing with before (B).
fn backdrop_override(before: bool, shown: &Settings) -> Option<(String, f32, f32)> {
    before.then(|| {
        (
            shown.wallpaper.clone(),
            shown.wallpaper_dim,
            shown.video_dim,
        )
    })
}

impl SettingsScreen {
    pub fn new() -> Self {
        Self {
            tab: 0,
            more: [false; 4],
            selected: 0,
            entered: get_time(),
            last_mouse: Vec2::from(mouse_position()),
            dragging: false,
            drag_field: None,
            held: 0.0,
            snapshot: None,
            preview: None,
            before: false,
            drawn_before: false,
            video: None,
            video_on: false,
        }
    }

    /// Swaps in another song for the live preview (main hands over the selected song), with
    /// its video and video nudge (seconds) if it has one.
    pub fn set_preview(&mut self, p: Preview, video: Option<(crate::video::Clip, f64)>) {
        self.preview = Some(p);
        self.video = video;
    }

    fn switch(&mut self, tab: usize) {
        self.tab = tab;
        self.selected = 0;
        self.dragging = false;
    }

    fn toggle_more(&mut self) {
        self.more[self.tab] = !self.more[self.tab];
    }

    pub fn update(&mut self, s: &mut Settings) -> Exit {
        if self.snapshot.is_none() {
            self.snapshot = Some(s.clone());
            crate::wallpaper::keep_before(Some(&s.wallpaper));
        }
        if self.preview.is_none() {
            self.preview = Some(Preview::demo(s));
        }
        self.before = is_key_down(KeyCode::B);
        if is_key_pressed(KeyCode::R)
            && let Some(snapshot) = &self.snapshot
        {
            *s = snapshot.clone();
        }
        if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::Tab) {
            s.save();
            return Exit::Back;
        }
        if is_key_pressed(KeyCode::C) {
            return Exit::Calibrate;
        }
        let step = i32::from(is_key_pressed(KeyCode::E)) - i32::from(is_key_pressed(KeyCode::Q));
        if step != 0 {
            self.switch((self.tab as i32 + step).rem_euclid(TABS.len() as i32) as usize);
        }
        let mouse = Vec2::from(mouse_position());
        let head = layout(entries(self.tab, self.more[self.tab]).len());
        if is_mouse_button_pressed(MouseButton::Left)
            && (mouse.y - head.tabs_y).abs() < 16.0 * head.scale
            && let Some(i) = (0..TABS.len()).find(|i| {
                let (x, w) = tab_span(*i, head.scale);
                mouse.x >= x && mouse.x <= x + w
            })
        {
            self.switch(i);
        }
        let list = entries(self.tab, self.more[self.tab]);
        let n = list.len();
        self.selected = self.selected.min(n - 1);
        if is_key_pressed(KeyCode::Down) {
            self.selected = (self.selected + 1) % n;
        }
        if is_key_pressed(KeyCode::Up) {
            self.selected = (self.selected + n - 1) % n;
        }
        let (_, wheel) = mouse_wheel();
        if wheel != 0.0 {
            self.selected =
                (self.selected as i32 - wheel.signum() as i32).clamp(0, n as i32 - 1) as usize;
        }
        let lay = layout(n);
        let over = (mouse.y >= lay.top && mouse.x >= 48.0 && mouse.x <= lay.right)
            .then(|| ((mouse.y - lay.top) / lay.row_h) as usize)
            .filter(|r| *r < n);
        if let Some(r) = over
            && mouse != self.last_mouse
            && !self.dragging
        {
            self.selected = r;
        }
        self.last_mouse = mouse;
        if is_mouse_button_pressed(MouseButton::Left)
            && let Some(r) = over
        {
            self.selected = r;
            match list[r] {
                Entry::Calibrate => return Exit::Calibrate,
                Entry::BatterySaver => s.battery_saver(),
                Entry::More => {
                    self.toggle_more();
                    return Exit::Stay;
                }
                Entry::Field(f) => {
                    if is_choice(f) {
                        s.nudge(row_of(f), 1.0, 1.0);
                    } else if mouse.x >= lay.track0 - 12.0 && mouse.x <= lay.track1 + 12.0 {
                        self.dragging = true;
                        self.drag_field = Some(f);
                    }
                }
            }
        }
        if !is_mouse_button_down(MouseButton::Left) {
            self.dragging = false;
        }
        let entry = list[self.selected];
        if self.dragging
            && let Some(f) = self.drag_field
        {
            let r = row_of(f);
            let t = ((mouse.x - lay.track0) / (lay.track1 - lay.track0)).clamp(0.0, 1.0);
            let raw = r.min + t * (r.max - r.min);
            s.set(f, (raw / r.step).round() * r.step);
        }
        // pressed as well as down: a tap shorter than one frame is released before it is read
        let held = |k: KeyCode| is_key_down(k) || is_key_pressed(k);
        let dir = i32::from(held(KeyCode::Right)) - i32::from(held(KeyCode::Left));
        if dir == 0 {
            self.held = 0.0;
        } else {
            let pressed = is_key_pressed(KeyCode::Right) || is_key_pressed(KeyCode::Left);
            let mut fire = pressed;
            if pressed {
                self.held = 0.0;
            } else {
                self.held += get_frame_time();
                if self.held >= REPEAT_DELAY {
                    self.held -= REPEAT_RATE;
                    fire = true;
                }
            }
            if fire {
                let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
                match entry {
                    Entry::Field(f) => {
                        s.nudge(row_of(f), dir as f32, if shift { 5.0 } else { 1.0 })
                    }
                    Entry::More if pressed => {
                        self.toggle_more();
                        return Exit::Stay;
                    }
                    Entry::BatterySaver if pressed => s.battery_saver(),
                    _ => {}
                }
            }
        }
        if is_key_pressed(KeyCode::Enter) {
            match entry {
                Entry::Field(f) if is_choice(f) => s.nudge(row_of(f), 1.0, 1.0),
                Entry::More => {
                    self.toggle_more();
                    return Exit::Stay;
                }
                Entry::Calibrate => return Exit::Calibrate,
                Entry::BatterySaver => s.battery_saver(),
                Entry::Field(_) => {}
            }
        }
        if is_key_pressed(KeyCode::Backspace)
            && let Entry::Field(f) = entry
        {
            s.reset_field(f);
        }
        if is_key_pressed(KeyCode::Delete) {
            s.reset_tab(self.tab);
        }
        Exit::Stay
    }

    pub fn draw(&mut self, skin: &Skin, fx: &mut PostFx, s: &Settings) {
        let cam = fx.begin(&render::camera(Vec2::ZERO, s));
        render::draw_space(skin, &cam, crate::palette()[1], s.space, s.approach_rate);
        gl_use_default_material();
        fx.present();
        let text = Text::screen(skin);
        let list = entries(self.tab, self.more[self.tab]);
        let lay = layout(list.len());
        let accent = crate::look::color(crate::look::get(s.look).notes[0]);
        let ac = |a: f32| Color::new(accent.r, accent.g, accent.b, a);
        let intro = (1.0 - (-((get_time() - self.entered) as f32) * 8.0).exp()).clamp(0.0, 1.0);
        let tint = |a: f32| Color::new(1.0, 1.0, 1.0, a * intro);
        let slide = (1.0 - intro) * 12.0;
        let (title_y, title_px, sub_y) = header(lay.scale);
        text.draw(
            "settings",
            vec2(64.0 + slide, title_y),
            Style::new(title_px, tint(1.0)).bold(true),
        );
        text.draw(
            "Esc saves and goes back",
            vec2(66.0 + slide, sub_y),
            Style::new(13.0, tint(0.45)),
        );
        for (i, name) in TAB_NAMES.iter().enumerate() {
            let (x, tw) = tab_span(i, lay.scale);
            let on = i == self.tab;
            text.draw(
                name,
                vec2(x + slide, lay.tabs_y),
                Style::new(20.0, tint(if on { 1.0 } else { 0.45 }))
                    .bold(on)
                    .middle(),
            );
            if on {
                draw_rectangle(x, lay.tabs_y + 16.0 * lay.scale, tw * 0.55, 3.0, ac(intro));
            }
        }
        for (i, e) in list.iter().enumerate() {
            let y = lay.top + i as f32 * lay.row_h;
            let center = y + (lay.row_h - 6.0) / 2.0;
            let on = i == self.selected;
            let (label, value) = match e {
                Entry::Field(f) => (row_of(*f).label, s.show(*f)),
                Entry::Calibrate => ("Calibrate...", "Enter".to_string()),
                Entry::BatterySaver => (
                    "Battery saver",
                    if s.is_battery_saver() {
                        "on".to_string()
                    } else {
                        "Enter to apply".to_string()
                    },
                ),
                Entry::More => (
                    if self.more[self.tab] {
                        "Fewer options"
                    } else {
                        "More options..."
                    },
                    String::new(),
                ),
            };
            if on {
                draw_rectangle(48.0, y, lay.right - 48.0, lay.row_h - 6.0, ac(0.07 * intro));
                draw_rectangle(48.0, y, 3.0, lay.row_h - 6.0, ac(intro));
            }
            let color = tint(if on { 1.0 } else { 0.6 });
            text.draw(
                label,
                vec2(lay.label_x, center),
                Style::new(18.0, color).bold(on).middle(),
            );
            if let Entry::Field(f) = e {
                if s.differs(*f) {
                    draw_circle(lay.label_x - 16.0, center, 3.5, ac(intro));
                }
                if !is_choice(*f) {
                    let r = row_of(*f);
                    let t = ((s.get(*f) - r.min) / (r.max - r.min)).clamp(0.0, 1.0);
                    let (x0, x1) = (lay.track0, lay.track1);
                    draw_rectangle(x0, center - 2.0, x1 - x0, 4.0, tint(0.14));
                    draw_rectangle(
                        x0,
                        center - 2.0,
                        (x1 - x0) * t,
                        4.0,
                        ac(intro * if on { 1.0 } else { 0.7 }),
                    );
                    draw_circle(
                        x0 + (x1 - x0) * t,
                        center,
                        if on { 7.0 } else { 5.0 },
                        tint(if on { 1.0 } else { 0.8 }),
                    );
                }
                if *f == Field::Look {
                    for (k, hex) in crate::look::get(s.look).notes.iter().enumerate() {
                        let mut c = crate::look::color(*hex);
                        c.a = intro;
                        draw_rectangle(lay.track0 + k as f32 * 26.0, center - 9.0, 20.0, 18.0, c);
                    }
                }
            }
            let value_style = Style {
                align: Align::Right,
                ..Style::new(18.0, color)
            };
            text.draw(
                &value,
                vec2(lay.value_x, center),
                value_style.bold(on).middle(),
            );
        }
        self.draw_preview(skin, &text, s, lay.preview, intro, accent);
        let hint = "Q/E tabs   Up/Down select   Left/Right adjust (Shift x5)   Backspace reset   Delete reset tab   R undo   C calibrate   Esc back";
        text.draw(
            hint,
            vec2(64.0, screen_height() - 56.0),
            Style::new(14.0, tint(0.55)),
        );
        if self.tab == 1 {
            let appear = format!(
                "notes appear {:.2}s before the hit",
                s.approach_distance / s.approach_rate
            );
            text.draw(
                &appear,
                vec2(64.0, screen_height() - 28.0),
                Style::new(13.0, tint(0.3)),
            );
        }
    }

    /// Advances and draws the live preview. While B is held it shows the settings as they
    /// were when the screen opened. With no room on screen the preview pauses.
    fn draw_preview(
        &mut self,
        skin: &Skin,
        text: &Text,
        s: &Settings,
        area: Option<Rect>,
        intro: f32,
        accent: Color,
    ) {
        let Some(p) = &mut self.preview else { return };
        let Some(area) = area else {
            p.hide();
            return;
        };
        let shown = match &self.snapshot {
            Some(before) if self.before => before,
            _ => s,
        };
        if self.before != self.drawn_before {
            self.drawn_before = self.before;
            p.redraw();
        }
        crate::look::set_active(shown.look);
        // The backdrop is global wallpaper state synced from the live settings; while B is
        // held the preview must show the wallpaper and dims it was opened with.
        let before = backdrop_override(self.before, shown);
        crate::wallpaper::set_before(before.as_ref().map(|(n, d, v)| (n.as_str(), *d, *v)));
        // The song's own video behind the preview, following the live song video setting.
        let video_on = s.song_video && self.video.is_some();
        if video_on != self.video_on {
            self.video_on = video_on;
            let video = self.video.as_ref().filter(|_| video_on);
            crate::wallpaper::set_preview_song_video(video.map(|(clip, _)| clip));
            if let Some((_, nudge)) = video {
                crate::wallpaper::set_song_video_nudge(*nudge);
            }
        }
        p.update(shown);
        if self.video_on {
            crate::wallpaper::song_video_time(p.song_time());
        }
        crate::wallpaper::set_in_preview(true);
        p.draw(skin, area, s.preview_rate());
        crate::wallpaper::set_in_preview(false);
        crate::wallpaper::set_before(None);
        crate::look::set_active(s.look);
        let tint = |a: f32| Color::new(1.0, 1.0, 1.0, a * intro);
        text.draw(
            "preview",
            vec2(area.x, area.y - 10.0),
            Style::new(13.0, tint(0.45)),
        );
        text.draw(
            if self.before {
                "showing the settings you opened with"
            } else {
                "hold B to compare with before"
            },
            vec2(area.x + area.w, area.y - 10.0),
            Style {
                align: Align::Right,
                ..Style::new(13.0, tint(0.3))
            },
        );
        if self.before {
            let pad = 10.0;
            draw_rectangle(
                area.x + pad,
                area.y + pad,
                78.0,
                22.0,
                Color::new(0.0, 0.0, 0.0, 0.7),
            );
            draw_rectangle(area.x + pad, area.y + pad, 3.0, 22.0, accent);
            text.draw(
                "BEFORE",
                vec2(area.x + pad + 12.0, area.y + pad + 11.0),
                Style::new(13.0, accent).bold(true).middle(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_fields() -> Vec<Field> {
        ROWS.iter().map(|r| r.field).collect()
    }

    #[test]
    fn every_field_sits_in_exactly_one_tab() {
        for f in all_fields() {
            let hits = TABS
                .iter()
                .map(|t| {
                    t.main
                        .iter()
                        .chain(t.more.iter())
                        .filter(|x| **x == f)
                        .count()
                })
                .sum::<usize>();
            assert_eq!(hits, 1, "{f:?}");
        }
        let in_tabs: usize = TABS.iter().map(|t| t.main.len() + t.more.len()).sum();
        assert_eq!(in_tabs, all_fields().len());
    }

    #[test]
    fn more_hides_technical_rows_and_calibrate_lives_in_sound() {
        assert_eq!(entries(0, false).len(), TABS[0].main.len() + 1);
        assert_eq!(
            entries(0, true).len(),
            TABS[0].main.len() + 1 + TABS[0].more.len()
        );
        assert!(entries(2, false).contains(&Entry::Calibrate));
        assert!(!entries(1, true).contains(&Entry::Calibrate));
        assert!(!entries(2, false).contains(&Entry::Field(Field::AudioBuffer)));
        assert!(entries(2, true).contains(&Entry::Field(Field::AudioBuffer)));
    }

    #[test]
    fn a_look_changes_only_look_and_background() {
        let mut s = Settings {
            sensitivity: 1.7,
            offset_ms: 12.0,
            music_volume: 0.3,
            audio_buffer: 512.0,
            ..Settings::default()
        };
        let before = s.clone();
        s.nudge(row_of(Field::Look), 1.0, 1.0);
        assert_eq!(s.look, 1);
        assert_eq!(s.space, crate::look::get(1).space);
        s.look = before.look;
        s.space = before.space;
        assert_eq!(s, before);
    }

    #[test]
    fn look_nudging_wraps_and_competitive_changes_only_look_and_space() {
        let mut wrapped = Settings::default();
        for _ in 0..9 {
            wrapped.nudge(row_of(Field::Look), 1.0, 1.0);
        }
        assert_eq!(wrapped.look, 0);

        let mut s = Settings {
            sensitivity: 1.7,
            offset_ms: 12.0,
            music_volume: 0.3,
            sfx_volume: 0.6,
            audio_buffer: 512.0,
            note_core: true,
            glow: false,
            cursor_trail: true,
            wallpaper: "aim.png".into(),
            ..Settings::default()
        };
        let before = s.clone();
        for _ in 0..crate::look::LOOKS.len() - 1 {
            s.nudge(row_of(Field::Look), 1.0, 1.0);
        }
        assert_eq!(s.look, crate::look::LOOKS.len() - 1);
        assert_eq!(s.space, Space::Void);
        assert_eq!(s.sensitivity, before.sensitivity);
        assert_eq!(s.offset_ms, before.offset_ms);
        assert_eq!(s.music_volume, before.music_volume);
        assert_eq!(s.sfx_volume, before.sfx_volume);
        assert_eq!(s.audio_buffer, before.audio_buffer);
        assert_eq!(s.note_core, before.note_core);
        assert_eq!(s.glow, before.glow);
        assert_eq!(s.cursor_trail, before.cursor_trail);
        assert_eq!(s.wallpaper, before.wallpaper);
    }

    #[test]
    fn saved_settings_round_trip_with_the_new_fields() {
        let s = Settings {
            sensitivity: 1.25,
            look: 3,
            wallpaper: "city.png".into(),
            wallpaper_dim: 0.4,
            space: Space::Void,
            song_video: false,
            song_preview: false,
            game_fps: 4,
            menu_fps: 3,
            video_fps: 3,
            preview_fps: 2,
            video_quality: 2,
            video_blur: 1,
            video_dim: 0.3,
            show_fps: true,
            ..Settings::default()
        };
        let text = s.to_text();
        assert!(text.contains(r#""game_fps": "144""#), "{text}");
        assert!(text.contains(r#""video_quality": "1080""#), "{text}");
        assert!(text.contains(r#""video_blur": "light""#), "{text}");
        assert_eq!(Settings::parse(&text), Some(s));
        let on = Settings::default();
        assert_eq!(Settings::parse(&on.to_text()), Some(on));
        // every choice of every list row survives a save
        for r in &ROWS {
            for (i, _) in choices(r.field).unwrap_or_default().iter().enumerate() {
                let mut s = Settings::default();
                s.set(r.field, i as f32);
                assert_eq!(
                    Settings::parse(&s.to_text()).unwrap().get(r.field),
                    i as f32
                );
            }
        }
        // hand-edited numbers and unknown names
        let typed = Settings::parse(
            r#"{"video_quality": 720, "game_fps": "Unlimited", "menu_fps": "45", "video_dim": 5}"#,
        )
        .unwrap();
        assert_eq!((typed.video_quality, typed.game_fps), (1, 0));
        assert_eq!(typed.menu_fps, Settings::default().menu_fps);
        assert_eq!(typed.video_dim, 0.9);
    }

    #[test]
    fn frame_rate_settings_map_to_caps() {
        let with = |game_fps: usize, menu_fps: usize| Settings {
            game_fps,
            menu_fps,
            ..Settings::default()
        };
        let d = Settings::default();
        // defaults are the old behaviour: vsync in game, 11.5 ms menu sleep
        assert!(d.vsync());
        assert_eq!(d.game_cap(), Cap::Vsync);
        assert_eq!(d.menu_cap(true), Cap::Delay(11.5));
        assert_eq!(d.menu_cap(false), Cap::Fps(72.0));
        let games = [
            Cap::Unlimited,
            Cap::Vsync,
            Cap::Fps(60.0),
            Cap::Fps(120.0),
            Cap::Fps(144.0),
            Cap::Fps(240.0),
            Cap::Fps(360.0),
        ];
        for (i, cap) in games.into_iter().enumerate() {
            assert_eq!(with(i, 3).game_cap(), cap);
            assert_eq!(with(i, 3).menu_cap(true), cap, "menus follow the game");
            assert_eq!(with(i, 0).vsync(), i == 1);
        }
        assert_eq!(with(1, 0).menu_cap(true), Cap::Fps(30.0));
        assert_eq!(with(1, 1).menu_cap(false), Cap::Fps(60.0));
        // and each cap turns into the sleep the frame loop expects
        let mut l = crate::perf::Limiter::default();
        assert_eq!(l.delay_ms(with(0, 0).game_cap(), 2.0, false, 4.0), 0.0);
        assert_eq!(
            l.delay_ms(with(1, 0).game_cap(), 6.9, true, 4.0),
            4.0,
            "pacer under vsync"
        );
        assert_eq!(l.delay_ms(d.menu_cap(true), 13.9, true, 0.0), 11.5);
        let mut delay = 0.0;
        for _ in 0..1000 {
            delay = l.delay_ms(with(2, 0).game_cap(), 1.0 + delay, false, 0.0);
        }
        assert!((delay - (1000.0 / 60.0 - 1.0)).abs() < 0.05, "{delay}");
    }

    #[test]
    fn video_options_and_battery_saver() {
        let d = Settings::default();
        assert_eq!(
            d.video_options(),
            crate::video::Options {
                height: 540,
                fps: Some(30.0),
                blur: 0
            }
        );
        assert_eq!(d.preview_rate(), Some(60.0));
        let s = Settings {
            video_quality: 2,
            video_fps: 3,
            video_blur: 2,
            preview_fps: 2,
            ..d.clone()
        };
        assert_eq!(
            s.video_options(),
            crate::video::Options {
                height: 1080,
                fps: None,
                blur: 2
            }
        );
        assert_eq!(s.preview_rate(), None);
        let mut saver = s.clone();
        assert!(!saver.is_battery_saver());
        saver.battery_saver();
        assert!(saver.is_battery_saver());
        assert_eq!(
            (
                saver.menu_cap(true),
                saver.video_options().fps,
                saver.preview_rate(),
                saver.glow
            ),
            (Cap::Fps(30.0), Some(24.0), Some(30.0), false)
        );
        assert_eq!(
            (saver.video_quality, saver.game_fps),
            (s.video_quality, s.game_fps)
        );
        assert!(entries(3, false).contains(&Entry::BatterySaver));
    }

    #[test]
    fn before_preview_uses_the_snapshot_wallpaper_and_dims() {
        let snap = Settings {
            wallpaper: "old.png".into(),
            wallpaper_dim: 0.2,
            video_dim: 0.7,
            ..Settings::default()
        };
        assert_eq!(
            backdrop_override(true, &snap),
            Some(("old.png".to_string(), 0.2, 0.7))
        );
        assert_eq!(backdrop_override(false, &snap), None);
    }

    #[test]
    fn an_old_settings_file_still_loads() {
        let s = Settings::parse(r#"{"sensitivity": 1.5, "glow": false}"#).unwrap();
        assert_eq!(
            (s.look, s.wallpaper.as_str(), s.wallpaper_dim),
            (0, "", 0.55)
        );
        assert_eq!((s.sensitivity, s.glow), (1.5, false));
        assert_eq!((s.song_video, s.song_preview), (true, true));
        let d = Settings::default();
        assert_eq!(
            (
                s.game_fps,
                s.menu_fps,
                s.video_fps,
                s.preview_fps,
                s.show_fps
            ),
            (d.game_fps, d.menu_fps, d.video_fps, d.preview_fps, false)
        );
        assert_eq!((s.video_quality, s.video_blur, s.video_dim), (0, 0, 0.55));
        let typo = Settings::parse(r#"{"song_video": 1, "song_preview": false}"#).unwrap();
        assert_eq!((typo.song_video, typo.song_preview), (true, false));
        assert_eq!(Settings::parse("not json"), None);
        let wild = Settings::parse(r#"{"look": 99, "wallpaper_dim": 7}"#).unwrap();
        assert_eq!(wild.look, crate::look::LOOKS.len() - 1);
        assert_eq!(wild.wallpaper_dim, 0.85);
    }

    #[test]
    fn wallpaper_cycling_wraps_with_and_without_files() {
        assert_eq!(next_wallpaper("", &[], 1), "");
        assert_eq!(next_wallpaper("gone.png", &[], -1), "");
        let names = vec!["a.png".to_string(), "b.png".to_string()];
        assert_eq!(next_wallpaper("", &names, 1), "a.png");
        assert_eq!(next_wallpaper("a.png", &names, 1), "b.png");
        assert_eq!(next_wallpaper("b.png", &names, 1), "");
        assert_eq!(next_wallpaper("", &names, -1), "b.png");
    }

    #[test]
    fn resetting_a_row_or_a_tab_touches_only_what_it_should() {
        let mut s = Settings {
            sensitivity: 2.0,
            look: 3,
            wallpaper: "x.png".into(),
            cursor_trail: true,
            ..Settings::default()
        };
        s.reset_field(Field::Sensitivity);
        assert_eq!(s.sensitivity, Settings::default().sensitivity);
        assert_eq!(s.look, 3);
        s.sensitivity = 2.0;
        s.reset_tab(0);
        assert_eq!(
            (s.look, s.wallpaper.as_str(), s.cursor_trail),
            (0, "", false)
        );
        assert_eq!(s.sensitivity, 2.0);
    }

    #[test]
    fn the_changed_dot_follows_the_default() {
        let mut s = Settings::default();
        assert!(all_fields().iter().all(|f| !s.differs(*f)));
        s.sensitivity = 1.0;
        s.wallpaper = "a.png".into();
        assert!(s.differs(Field::Sensitivity) && s.differs(Field::Wallpaper));
        assert!(!s.differs(Field::Fov));
    }

    #[test]
    fn rows_fit_the_screen_in_every_tab_at_common_sizes() {
        for (w, h) in [
            (1280.0, 720.0),
            (1920.0, 1080.0),
            (1000.0, 600.0),
            (800.0, 480.0),
        ] {
            for tab in 0..TABS.len() {
                let n = entries(tab, true).len();
                let lay = layout_for(w, h, n);
                let bottom = lay.top + n as f32 * lay.row_h - 6.0;
                assert!(
                    bottom <= h - 70.0,
                    "{w}x{h} tab {tab}: rows end at {bottom}"
                );
                assert!(lay.tabs_y < lay.top - 20.0);
                let (_, _, sub_y) = header(lay.scale);
                assert!(
                    sub_y < lay.tabs_y - 10.0 * lay.scale - 8.0,
                    "{w}x{h}: tabs over the header"
                );
                assert!(
                    lay.track1 < lay.value_x - 40.0,
                    "{w}x{h}: slider runs into the value"
                );
                let (x, tw) = tab_span(TABS.len() - 1, lay.scale);
                assert!(x + tw < w);
                if let Some(p) = lay.preview {
                    assert!(p.w >= 160.0 && p.h >= 90.0, "{w}x{h}: preview {p:?}");
                    assert!(
                        p.x + p.w <= w - 40.0 && p.y + p.h <= h - 70.0,
                        "{w}x{h}: {p:?}"
                    );
                    let beside = p.x >= lay.right + 16.0;
                    let under = p.y >= bottom + 8.0;
                    assert!(beside || under, "{w}x{h} tab {tab}: preview covers rows");
                    assert!(p.y > lay.tabs_y + 20.0);
                }
            }
        }
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (1000.0, 600.0)] {
            let lay = layout_for(w, h, entries(0, true).len());
            assert!(lay.preview.is_some_and(|p| p.x > lay.right), "{w}x{h}");
        }
        let tall = layout_for(800.0, 1000.0, entries(0, true).len());
        assert!(
            tall.preview.is_some_and(|p| p.y > tall.top),
            "narrow: preview under rows"
        );
    }
}
