use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

use macroquad::prelude::{
    BLACK, Camera2D, Camera3D, Color, DrawTextureParams, Rect, Texture2D, Vec2, clear_background,
    draw_rectangle, draw_texture_ex, screen_height, screen_width, set_camera, vec2,
};
use macroquad::texture::Image;

/// Encoded image formats enabled by macroquad's `image` features.
pub const EXTS: [&str; 3] = ["png", "jpg", "jpeg"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            EXTS.iter()
                .any(|supported| ext.eq_ignore_ascii_case(supported))
        })
}

pub fn is_wallpaper(path: &Path) -> bool {
    is_image(path) || crate::video::is_video(path)
}

pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_wallpaper(path))
        .collect();
    paths.sort_by_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });
    paths
}

/// A decoded wallpaper and its original pixel dimensions.
pub struct Wallpaper {
    texture: Texture2D,
    pub size: Vec2,
}

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_TEXTURE_SIDE: u32 = 2560;

fn check_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("Wallpaper has invalid zero dimensions".to_owned());
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!(
            "Wallpaper dimensions {width}x{height} exceed the 8192 px limit"
        ));
    }
    Ok(())
}

/// Reads PNG's IHDR dimensions before decoding, avoiding oversized allocations.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some((
        u32::from_be_bytes(bytes[16..20].try_into().ok()?),
        u32::from_be_bytes(bytes[20..24].try_into().ok()?),
    ))
}

/// Reads a JPEG's size from its frame header before decoding, so a tiny file that claims
/// a huge picture cannot make the decoder allocate gigabytes.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2;
    loop {
        while *bytes.get(at)? != 0xff {
            at += 1;
        }
        while *bytes.get(at)? == 0xff {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        match marker {
            0x01 | 0xd0..=0xd8 => continue,
            0xd9 | 0xda => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
        if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            let height = u16::from_be_bytes([*bytes.get(at + 3)?, *bytes.get(at + 4)?]);
            let width = u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]);
            return Some((u32::from(width), u32::from(height)));
        }
        if len < 2 {
            return None;
        }
        at += len;
    }
}

fn downscale(image: &mut Image) {
    let width = u32::from(image.width);
    let height = u32::from(image.height);
    let longest = width.max(height);
    if longest <= MAX_TEXTURE_SIDE {
        return;
    }
    let scale = MAX_TEXTURE_SIDE as f64 / longest as f64;
    let out_w = ((width as f64 * scale).round() as u32).max(1);
    let out_h = ((height as f64 * scale).round() as u32).max(1);
    let source = &image.bytes;
    let mut output = vec![0; (out_w * out_h * 4) as usize];
    for y in 0..out_h {
        let sy =
            ((y as f64 + 0.5) * height as f64 / out_h as f64 - 0.5).clamp(0.0, (height - 1) as f64);
        let y0 = sy.floor() as u32;
        let y1 = (y0 + 1).min(height - 1);
        let fy = sy - y0 as f64;
        for x in 0..out_w {
            let sx = ((x as f64 + 0.5) * width as f64 / out_w as f64 - 0.5)
                .clamp(0.0, (width - 1) as f64);
            let x0 = sx.floor() as u32;
            let x1 = (x0 + 1).min(width - 1);
            let fx = sx - x0 as f64;
            for channel in 0..4 {
                let sample =
                    |px: u32, py: u32| source[((py * width + px) * 4 + channel) as usize] as f64;
                let top = sample(x0, y0) * (1.0 - fx) + sample(x1, y0) * fx;
                let bottom = sample(x0, y1) * (1.0 - fx) + sample(x1, y1) * fx;
                output[((y * out_w + x) * 4 + channel) as usize] =
                    (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    image.bytes = output;
    image.width = out_w as u16;
    image.height = out_h as u16;
}

use crate::video::{Clip, Options, Video};

/// One playing video plus, while it is being reopened at new decode settings, the old one
/// (drawn until the new one has a frame, so a quality change never flashes the wallpaper).
struct Slot {
    clip: Clip,
    looping: bool,
    video: Video,
    old: Option<Video>,
    /// The decoder's error was logged.
    reported: bool,
}

impl Slot {
    fn open(clip: Clip, looping: bool, options: Options) -> Slot {
        Slot {
            video: Video::open(&clip, looping, options),
            clip,
            looping,
            old: None,
            reported: false,
        }
    }

    fn configure(&mut self, options: Options) {
        if self.video.options() != options {
            let fresh = Video::open(&self.clip, self.looping, options).without_fade();
            let previous = std::mem::replace(&mut self.video, fresh);
            if previous.ready() || self.old.is_none() {
                self.old = Some(previous);
            }
            self.reported = false;
        }
    }

    fn sync(&mut self, t: f64) {
        self.video.sync(t);
        if !self.reported
            && let Some(e) = self.video.error()
        {
            self.reported = true;
            eprintln!("video {:?}: {e}", self.clip.source);
        }
        if self.video.ready() {
            self.old = None;
        } else if let Some(old) = &mut self.old {
            old.sync(t);
        }
    }

    fn current(&self) -> Option<&Video> {
        Some(&self.video)
            .filter(|v| v.ready())
            .or_else(|| self.old.as_ref().filter(|v| v.ready()))
    }
}

/// A wallpaper other than the live one: the settings screen's "before" (hold B).
struct Alt {
    name: String,
    wall: Option<Wallpaper>,
    video: Option<Slot>,
}

/// Opens wallpaper `name` from the wallpaper folder: a looping video slot or an image.
fn open(name: &str, options: Options) -> (Option<Wallpaper>, Option<Slot>) {
    if name.is_empty() {
        return (None, None);
    }
    let file = Path::new(name).file_name().unwrap_or_default();
    let path = Path::new(FOLDER).join(file);
    if crate::video::is_video(&path) {
        let clip = Clip {
            source: crate::video::Source::File(path),
            offset: 0.0,
        };
        return (
            None,
            Some(Slot::open(clip, true, Options { blur: 0, ..options })),
        );
    }
    match Wallpaper::load(&path) {
        Ok(w) => (Some(w), None),
        Err(e) => {
            eprintln!("wallpaper {name}: {e}");
            (None, None)
        }
    }
}

#[derive(Default)]
struct Active {
    name: String,
    wall: Option<Wallpaper>,
    video: Option<Slot>,
    song: Option<Slot>,
    /// Song video offset: the clip's own plus the per-song nudge (seconds).
    song_offset: f64,
    dim: f32,
    video_dim: f32,
    options: Options,
    /// Wallpaper videos pause (their clock stops) while the window is unfocused.
    paused_at: Option<f64>,
    paused_total: f64,
    /// Settings preview "before": wallpaper name, wallpaper dim, video dim.
    before: Option<(String, f32, f32)>,
    /// The wallpaper (image or video) for `before`, loaded once per name.
    alt: Option<Alt>,
    /// While the settings screen is open: the wallpaper it opened with, kept as `alt` when
    /// the live wallpaper changes away from it.
    keep: Option<String>,
    /// The song video belongs to the settings preview: drawn only while `in_preview`.
    song_scoped: bool,
    in_preview: bool,
}

thread_local! {
    static ACTIVE: RefCell<Active> = RefCell::new(Active::default());
}

/// Opens (or clears) the song's music video with the current video settings.
pub fn set_song_video(clip: Option<&Clip>) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        let options = a.options;
        a.song_offset = clip.map_or(0.0, |c| c.offset);
        a.song = clip.map(|c| Slot::open(c.clone(), false, options));
        a.song_scoped = false;
    });
}

/// Opens (or clears) a song video shown only inside the settings preview, that is between
/// `set_in_preview(true)` and `set_in_preview(false)`; synced with `song_video_time`.
pub fn set_preview_song_video(clip: Option<&Clip>) {
    set_song_video(clip);
    ACTIVE.with(|cell| cell.borrow_mut().song_scoped = clip.is_some());
}

/// Marks the settings preview being drawn (a preview song video shows only then).
pub fn set_in_preview(on: bool) {
    ACTIVE.with(|cell| cell.borrow_mut().in_preview = on);
}

/// The settings screen opened with wallpaper `name` (Some) or closed (None). While open,
/// that wallpaper (image or playing video) stays loaded for "before" (B) even after the live
/// wallpaper changes; closing drops it.
pub fn keep_before(name: Option<&str>) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        a.keep = name.map(str::to_owned);
        if a.keep.is_none() {
            a.alt = None;
        }
    });
}

/// Extra per-song offset (seconds, positive = video later), on top of the clip's own.
pub fn set_song_video_nudge(extra: f64) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        if let Some(slot) = &a.song {
            let base = slot.clip.offset;
            a.song_offset = base + extra;
        }
    });
}

pub fn has_song_video() -> bool {
    ACTIVE.with(|cell| cell.borrow().song.is_some())
}

pub fn song_video_time(t: f64) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        let offset = a.song_offset;
        if let Some(slot) = &mut a.song {
            slot.sync(t - offset);
        }
    });
}

/// Music video look and decode settings; a change of quality, rate or blur reopens the
/// playing videos at the same time. Wallpaper videos are never blurred.
pub fn set_video_look(video_dim: f32, options: Options) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        a.video_dim = video_dim;
        a.options = options;
        if let Some(slot) = &mut a.song {
            slot.configure(options);
        }
        if let Some(slot) = &mut a.video {
            slot.configure(Options { blur: 0, ..options });
        }
        if let Some(slot) = a.alt.as_mut().and_then(|alt| alt.video.as_mut()) {
            slot.configure(Options { blur: 0, ..options });
        }
    });
}

/// Stops (or resumes) the clock of looping wallpaper videos.
pub fn set_paused(paused: bool) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        let now = macroquad::time::get_time();
        match (paused, a.paused_at) {
            (true, None) => a.paused_at = Some(now),
            (false, Some(at)) => {
                a.paused_total += now - at;
                a.paused_at = None;
            }
            _ => {}
        }
    });
}

/// While Some, `backdrop` shows this wallpaper and these dims instead (settings preview, B).
pub fn set_before(before: Option<(&str, f32, f32)>) {
    ACTIVE.with(|cell| {
        cell.borrow_mut().before = before.map(|(n, d, v)| (n.to_owned(), d, v));
    });
}

pub fn sync(name: &str, dim: f32) {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        a.dim = dim;
        let clock = a.paused_at.unwrap_or_else(macroquad::time::get_time) - a.paused_total;
        if let Some(slot) = &mut a.video {
            slot.sync(clock);
        }
        // The kept "before" video runs on the same clock, so B shows it at once and in step.
        if let Some(slot) = a.alt.as_mut().and_then(|alt| alt.video.as_mut()) {
            slot.sync(clock);
        }
        if a.name == name {
            return;
        }
        let old = Alt {
            name: std::mem::replace(&mut a.name, name.to_owned()),
            wall: a.wall.take(),
            video: a.video.take(),
        };
        // Back to the kept wallpaper: reuse it instead of loading it again.
        let reuse = a.alt.take_if(|alt| alt.name == name);
        if a.keep.as_deref() == Some(old.name.as_str()) {
            a.alt = Some(old);
        }
        (a.wall, a.video) = match reuse {
            Some(alt) => (alt.wall, alt.video),
            None => open(name, a.options),
        };
    });
}

pub fn backdrop(cam: &Camera3D) -> bool {
    ACTIVE.with(|cell| {
        let mut a = cell.borrow_mut();
        // "before" with another wallpaper not kept yet: open it once (image or video; a
        // video's first frame follows a frame later, once `sync` has run its clock).
        if let Some((name, _, _)) = &a.before
            && *name != a.name
            && a.alt.as_ref().is_none_or(|alt| alt.name != *name)
        {
            let name = name.clone();
            let (wall, video) = open(&name, a.options);
            a.alt = Some(Alt { name, wall, video });
        }
        let a = &*a;
        let (dim, video_dim, own) = match &a.before {
            Some((name, d, v)) => (*d, *v, *name == a.name),
            None => (a.dim, a.video_dim, true),
        };
        let song = a
            .song
            .as_ref()
            .filter(|_| !a.song_scoped || a.in_preview)
            .and_then(Slot::current);
        let (wall, wall_video) = if own {
            (a.wall.as_ref(), a.video.as_ref())
        } else {
            let alt = a.alt.as_ref();
            (
                alt.and_then(|alt| alt.wall.as_ref()),
                alt.and_then(|alt| alt.video.as_ref()),
            )
        };
        let wall_video = wall_video.and_then(Slot::current);
        if song.is_none() && wall_video.is_none() && wall.is_none() {
            return false;
        }
        let size = match &cam.render_target {
            Some(t) => vec2(t.texture.width(), t.texture.height()),
            None => vec2(screen_width(), screen_height()),
        };
        let mut flat = Camera2D::from_display_rect(Rect::new(0.0, 0.0, size.x, size.y));
        flat.render_target = cam.render_target.clone();
        set_camera(&flat);
        clear_background(BLACK);
        // The song video fades in over the wallpaper, so draw the wallpaper first.
        if let Some(video) = wall_video {
            video.draw(size, dim);
        } else if let Some(wall) = wall {
            wall.draw(size, dim, Vec2::ZERO);
        }
        if let Some(video) = song {
            video.draw(size, video_dim);
        }
        set_camera(cam);
        true
    })
}

pub const FOLDER: &str = "wallpapers";

/// Imports a wallpaper. Images are decoded into a texture first (render thread only); a video
/// is only copied, so main runs video imports on its importer thread (they can be large).
pub fn import(src: &Path) -> Result<String, String> {
    if crate::video::is_video(src) {
        if !src.is_file() {
            return Err("Wallpaper video is not a file".into());
        }
    } else {
        Wallpaper::load(src)?;
    }
    copy_in(Path::new(FOLDER), src)
}

fn copy_in(dir: &Path, src: &Path) -> Result<String, String> {
    let name = src
        .file_name()
        .ok_or_else(|| "no file name".to_string())?
        .to_owned();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let same = |a: &Path, b: &Path| same_file_contents(a, b);
    let mut candidate = dir.join(&name);
    let (stem, ext) = {
        let p = Path::new(&name);
        (
            p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            p.extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
        )
    };
    let mut n = 1;
    while candidate.exists() {
        if same(&candidate, src) {
            return Ok(candidate
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned());
        }
        n += 1;
        candidate = dir.join(format!("{stem} ({n}){ext}"));
    }
    std::fs::copy(src, &candidate).map_err(|e| e.to_string())?;
    Ok(candidate
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned())
}

/// Compares two files in small chunks after a size check, so a multi-GB video wallpaper
/// dropped twice is never read into memory.
fn same_file_contents(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut fa, mut fb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let n = match fa.read(&mut ba) {
            Ok(0) => return true,
            Ok(n) => n,
            Err(_) => return false,
        };
        if fb.read_exact(&mut bb[..n]).is_err() || ba[..n] != bb[..n] {
            return false;
        }
    }
}

impl Wallpaper {
    pub fn load(path: &Path) -> Result<Wallpaper, String> {
        if !is_image(path) {
            return Err("Unsupported wallpaper image format (PNG only is enabled)".to_owned());
        }
        let metadata = std::fs::metadata(path)
            .map_err(|err| format!("Could not read wallpaper metadata: {err}"))?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err("Wallpaper file exceeds the 64 MB limit".to_owned());
        }
        let bytes =
            std::fs::read(path).map_err(|err| format!("Could not read wallpaper: {err}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("Wallpaper file exceeds the 64 MB limit".to_owned());
        }
        if let Some((width, height)) = png_dimensions(&bytes).or_else(|| jpeg_dimensions(&bytes)) {
            check_dimensions(width, height)?;
        }
        let mut image = Image::from_file_with_format(&bytes, None)
            .map_err(|err| format!("Could not decode wallpaper: {err}"))?;
        check_dimensions(u32::from(image.width), u32::from(image.height))?;
        downscale(&mut image);
        let size = vec2(image.width as f32, image.height as f32);
        let texture = Texture2D::from_image(&image);
        Ok(Wallpaper { texture, size })
    }

    pub fn draw(&self, screen: Vec2, dim: f32, shift: Vec2) {
        let source = cover_uv((self.size.x, self.size.y), (screen.x, screen.y), shift);
        draw_texture_ex(
            &self.texture,
            0.0,
            0.0,
            Color::new(1.0, 1.0, 1.0, 1.0),
            DrawTextureParams {
                dest_size: Some(screen),
                source: Some(source),
                ..Default::default()
            },
        );
        let alpha = dim.clamp(0.0, 1.0);
        if alpha > 0.0 {
            draw_rectangle(
                0.0,
                0.0,
                screen.x,
                screen.y,
                Color::new(0.0, 0.0, 0.0, alpha),
            );
        }
    }
}

/// Computes the pixel-space crop to fill the screen without changing aspect ratio.
pub fn cover_uv(image: (f32, f32), screen: (f32, f32), shift: Vec2) -> Rect {
    let (iw, ih) = image;
    let (sw, sh) = screen;
    if !iw.is_finite()
        || !ih.is_finite()
        || !sw.is_finite()
        || !sh.is_finite()
        || iw <= 0.0
        || ih <= 0.0
        || sw <= 0.0
        || sh <= 0.0
    {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    }
    let (crop_w, crop_h) = if iw / ih > sw / sh {
        (ih * sw / sh, ih)
    } else {
        (iw, iw * sh / sw)
    };
    let max_x = (iw - crop_w).max(0.0);
    let max_y = (ih - crop_h).max(0.0);
    let sx = if shift.x.is_finite() {
        shift.x.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let sy = if shift.y.is_finite() {
        shift.y.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    Rect::new(
        max_x * (sx + 1.0) * 0.5,
        max_y * (sy + 1.0) * 0.5,
        crop_w,
        crop_h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rhythm-wallpaper-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01, "{a} != {b}");
    }

    #[test]
    fn recognizes_only_enabled_image_extensions() {
        assert!(is_image(Path::new("image.PNG")));
        assert!(is_image(Path::new("/x/photo.png")));
        for path in ["x.gif", "x.webp", "x.bmp", "x", "x.png.txt"] {
            assert!(!is_image(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn video_wallpapers_are_recognized_and_scanned() {
        let dir = temp_dir();
        for name in ["clip.MP4", "clip.webm", "clip.mkv", "no.mp3"] {
            std::fs::write(dir.join(name), []).unwrap();
        }
        assert!(is_wallpaper(&dir.join("clip.MP4")));
        assert!(!is_image(&dir.join("clip.MP4")));
        assert!(!is_wallpaper(&dir.join("no.mp3")));
        assert_eq!(scan(&dir).len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scans_and_sorts_images_case_insensitively() {
        let dir = temp_dir();
        for name in ["z.png", "A.PNG", "b.png", "ignore.gif"] {
            std::fs::write(dir.join(name), []).unwrap();
        }
        std::fs::create_dir(dir.join("folder.png")).unwrap();
        let names: Vec<_> = scan(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["A.PNG", "b.png", "z.png"]);
        assert!(scan(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cover_crop_wide_and_tall_images() {
        let wide = cover_uv((200.0, 100.0), (100.0, 200.0), Vec2::ZERO);
        close(wide.x, 75.0);
        close(wide.y, 0.0);
        close(wide.w, 50.0);
        close(wide.h, 100.0);
        let tall = cover_uv((100.0, 200.0), (200.0, 100.0), Vec2::ZERO);
        close(tall.x, 0.0);
        close(tall.y, 75.0);
        close(tall.w, 100.0);
        close(tall.h, 50.0);
    }

    #[test]
    fn cover_equal_aspect_shift_clamping_and_zero_sizes() {
        let equal = cover_uv((100.0, 50.0), (200.0, 100.0), Vec2::ZERO);
        close(equal.x, 0.0);
        close(equal.y, 0.0);
        close(equal.w, 100.0);
        close(equal.h, 50.0);
        let shifted = cover_uv((200.0, 100.0), (100.0, 100.0), vec2(5.0, -5.0));
        close(shifted.x, 100.0);
        close(shifted.y, 0.0);
        assert!(cover_uv((0.0, 100.0), (100.0, 100.0), Vec2::ZERO).w == 0.0);
        assert!(cover_uv((100.0, 100.0), (0.0, 100.0), Vec2::ZERO).h == 0.0);
    }

    #[test]
    fn rejects_large_files_and_png_dimensions_before_decode() {
        let dir = temp_dir();
        let oversized = dir.join("large.png");
        let file = std::fs::File::create(&oversized).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        let err = Wallpaper::load(&oversized).err().unwrap();
        assert!(err.contains("64 MB"), "{err}");

        let huge = dir.join("huge.png");
        let mut png = vec![0; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&8193u32.to_be_bytes());
        png[20..24].copy_from_slice(&1u32.to_be_bytes());
        std::fs::write(&huge, png).unwrap();
        let err = Wallpaper::load(&huge).err().unwrap();
        assert!(err.contains("8192 px"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46];
        jpeg.extend([0xff, 0xff, 0xc0, 0x00, 0x11, 0x08]);
        jpeg.extend(height.to_be_bytes());
        jpeg.extend(width.to_be_bytes());
        jpeg.extend([0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
        jpeg
    }

    #[test]
    fn jpeg_size_is_read_from_the_header_and_oversized_ones_are_refused() {
        assert_eq!(
            jpeg_dimensions(&jpeg_header(1920, 1080)),
            Some((1920, 1080))
        );
        assert_eq!(
            jpeg_dimensions(&jpeg_header(65535, 65535)),
            Some((65535, 65535))
        );
        assert_eq!(jpeg_dimensions(&jpeg_header(10, 10)[..12]), None);
        assert_eq!(jpeg_dimensions(b"not a jpeg"), None);
        assert_eq!(jpeg_dimensions(&[]), None);
        let dir = temp_dir();
        let huge = dir.join("huge.jpg");
        std::fs::write(&huge, jpeg_header(65535, 65535)).unwrap();
        let err = Wallpaper::load(&huge).err().unwrap();
        assert!(err.contains("8192 px"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn importing_copies_reuses_and_never_overwrites() {
        let root = temp_dir();
        let (src, wall) = (root.join("src"), root.join("wallpapers"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.png"), b"one").unwrap();
        assert_eq!(copy_in(&wall, &src.join("a.png")).unwrap(), "a.png");
        assert_eq!(copy_in(&wall, &src.join("a.png")).unwrap(), "a.png");
        std::fs::write(src.join("a.png"), b"two").unwrap();
        assert_eq!(copy_in(&wall, &src.join("a.png")).unwrap(), "a (2).png");
        assert_eq!(std::fs::read(wall.join("a.png")).unwrap(), b"one");
        assert_eq!(std::fs::read(wall.join("a (2).png")).unwrap(), b"two");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_contents_checks_length_then_streams_chunks() {
        let dir = temp_dir();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut tail = big.clone();
        *tail.last_mut().unwrap() ^= 1;
        let mut head = big.clone();
        head[0] ^= 1;
        for (name, body) in [
            ("a", &big[..]),
            ("b", &big[..]),
            ("tail", &tail[..]),
            ("head", &head[..]),
            ("short", &big[..299_999]),
        ] {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let same = |x: &str, y: &str| same_file_contents(&dir.join(x), &dir.join(y));
        assert!(same("a", "b"));
        assert!(!same("a", "tail"), "difference in the last chunk");
        assert!(!same("a", "head"));
        assert!(!same("a", "short") && !same("short", "a"));
        assert!(!same("a", "missing"));
        // A 3 GB sparse file of another length is refused from metadata alone (instantly).
        let huge = dir.join("huge");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(3 << 30)
            .unwrap();
        let started = std::time::Instant::now();
        assert!(!same("a", "huge"));
        assert!(started.elapsed().as_millis() < 50);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
