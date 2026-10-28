# Video backgrounds + live preview: shared contract

> **Status: done.** The contract below is the original plan; the shipped API differs in places:
> `video::available() -> Option<bool>` (never blocks; `start_probe()` runs the ffmpeg check on a
> thread at startup), `Video::open(clip, looping, Options { height, fps, blur })`,
> `wallpaper::set_song_video(Option<&Clip>)` + `set_song_video_nudge(seconds)` (per-song offset from
> `offsets::VideoOffsets`, `,` / `.` in game), `wallpaper::set_video_look(video_dim, options)` (a
> change reopens the playing videos at the same time), `wallpaper::set_paused` (unfocused window) and
> `wallpaper::set_before` (settings preview while B is held). Zip extractions in `videos/` replace
> older versions of the same entry and are capped at 2 GB, least recently used first. Song select
> loads through one loader thread (newest request wins, stale generations dropped, failures not
> retried until the selection moves). Frame rate: `perf::Cap` from `Settings::game_cap` /
> `menu_cap`, turned into a sleep by `perf::Limiter`; vsync follows `game_fps` at runtime through
> the miniquad patch `window::set_swap_interval`, and `window::is_focused` drives the 15 fps
> background rate.

Three workers share this checkout. Each owns the files listed; never edit another owner's file.
No git commit / stash / reset / checkout. Keep `cargo check` green after every step (others build too).

## Owners

| Owner | Files |
|---|---|
| Video helper | `src/video.rs` (new), `src/map.rs`, `src/wallpaper.rs`, `src/import.rs` |
| Preview helper | `src/preview.rs` (new), `src/settings.rs`, `src/render.rs`, `src/postfx.rs` |
| Lead | `src/main.rs`, `src/game.rs`, `src/audio.rs`, `Cargo.toml`, `README.md` |

New `mod` lines in `src/main.rs`: the lead adds `mod video;` and `mod preview;` up front, so both files exist as stubs from the start.

## Video (`src/video.rs`)

Decoding runs `ffmpeg` as a child process (already a soft dependency in `audio.rs::via_ffmpeg`), streaming raw RGBA frames over stdout on its own thread. No new crates.

```rust
pub const EXTS: &[&str];                      // mp4 m4v webm mkv mov avi
pub fn is_video(path: &Path) -> bool;         // by extension
pub fn available() -> bool;                   // ffmpeg + ffprobe on PATH, cached

pub enum Source { File(PathBuf), Zip { archive: PathBuf, entry: String } }
pub struct Clip { pub source: Source, pub offset: f64 } // offset: song time (s) where video t=0 sits

pub struct Video;
impl Video {
    pub fn open(clip: &Clip, looping: bool) -> Video;   // never blocks: probe/extract/decode on a thread
    pub fn sync(&mut self, t: f64);                     // media time; seeks on jumps/backwards, uploads newest frame <= t; t<0 => nothing shown
    pub fn ready(&self) -> bool;                        // a frame is on screen
    pub fn error(&self) -> Option<String>;
    pub fn draw(&self, screen: Vec2, dim: f32);         // cover-fit like Wallpaper::draw
}
// Drop kills the ffmpeg child and joins/detaches the thread.
```

- Output size: fit inside 960x540 (keep aspect, even dims), fps capped at 30 (`-vf scale,fps`).
- One texture, updated in place (`Texture2D::update_from_bytes` or equivalent); never allocate per frame.
- Sync: decoder runs ahead at most a few frames (bounded channel). If `t` jumps backwards or more than ~1 s ahead, restart ffmpeg with `-ss t`. Paused = same `t` each frame = nothing happens.
- Looping: `t` wraps modulo duration (from ffprobe).
- Zip sources extract the entry once into `videos/` (cache, name from archive stem + entry hash), on the worker thread.
- Missing ffmpeg: `error()` says so, `draw` draws nothing, game keeps working.

## Map (`src/map.rs`)

- `Map` gains `pub video: Option<crate::video::Clip>`; `Map::new` keeps its signature (video = None), add `with_video`.
- `.osu` / `.osz`: parse `[Events]` `Video,<offset ms>,"file"` (and `1,<offset>,"file"`). `.osz` -> `Source::Zip`, loose `.osu` -> `Source::File` next to it.
- Song files that are videos (`mp4`, `m4v`, `webm`, `mkv`, `mov`) -> `Source::File(path)`, offset 0, but only when ffprobe says it has a video stream (do the probe lazily in `Video`, not in `Map::load`: `Map::load` runs on the meta thread for every file).
- `AUDIO_EXTS` gains `m4v webm mkv mov` if kira/ffmpeg can decode the audio (audio.rs already falls back to ffmpeg).

## Wallpaper (`src/wallpaper.rs`)

- Wallpapers may be videos: `scan`, `import`, `sync(name, dim)` accept `video::is_video` files; video wallpapers loop silently, timed by `get_time()`.
- Song video slot, used by the lead from `game.rs`/`main.rs`:
  ```rust
  pub fn set_song_video(video: Option<Video>);
  pub fn song_video_time(t: f64);   // song time; slot applies the clip offset itself
  ```
  `set_song_video` takes the `Video` plus the clip offset if needed (adjust signature to `Option<(Video, f64)>` if simpler, tell the lead).
- `backdrop` priority: song video (when ready) > wallpaper (image or video) > none. Dim = `wallpaper_dim`.
- `pub fn is_wallpaper(path) -> bool` = image or video.

## Settings (`src/settings.rs`)

New fields (with defaults, saved/loaded, old files still load):
- `song_video: bool` = true  — play the song's music video behind the game.
- `song_preview: bool` = true — play a clip of the selected song in song select.

Both in the Look tab (song_video) and Sound tab (song_preview). Lead reads them in `main.rs`/`game.rs`.

## Preview (`src/preview.rs`)

```rust
pub struct Preview;
impl Preview {
    pub fn demo(settings: &Settings) -> Preview;                                  // built-in demo map + audio
    pub fn new(map: Map, track: Option<StaticSoundData>, settings: &Settings) -> Preview;
    pub fn update(&mut self, settings: &Settings);  // mirrors settings live, loops a ~12 s window
    pub fn draw(&mut self, skin: &Skin, area: Rect);
}
```
Built on `Game` in autoplay via lead's `Game::preview(&mut self, settings, window)` (game.rs) and `Music::set_volumes` (audio.rs).
`SettingsScreen` owns an `Option<Preview>`; `SettingsScreen::set_preview(&mut self, p: Preview)` lets main swap in the selected song.
Layout: preview panel to the right of the rows on wide screens, under them on narrow ones; rows must still fit (`rows_fit_the_screen_in_every_tab_at_common_sizes`).
Hold `B` = before: preview renders with the settings as they were when the screen opened (snapshot).

## Lead (`main.rs`, `game.rs`, `audio.rs`)

- `Game::preview`, `Music::set_volumes`.
- Song video: on load, if `settings.song_video` and `map.video`, open it into the wallpaper slot; feed song time every frame; clear on exit.
- Song select: background-load the selected entry; play a faded clip from ~40 % in (when `song_preview`); show its video muted behind the list; hand the loaded song to the settings preview.
- Drag and drop: video files import as songs; hold Shift while dropping to make it the wallpaper instead.
