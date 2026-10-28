//! Bounded, silent ffmpeg decoding; process and filesystem work lives on the worker.
use macroquad::prelude::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

pub const EXTS: &[&str] = &["mp4", "m4v", "webm", "mkv", "mov", "avi"];
pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| EXTS.iter().any(|e| x.eq_ignore_ascii_case(e)))
}
/// ffmpeg/ffprobe next to the AppImage or the program win over PATH, so a portable folder
/// can carry its own copy.
fn tool(name: &str) -> PathBuf {
    let beside_appimage = std::env::var_os("APPIMAGE")
        .and_then(|image| Path::new(&image).parent().map(Path::to_path_buf));
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    tool_in(name, beside_appimage.into_iter().chain(beside_exe))
}
fn tool_in(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    dirs.into_iter()
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}
pub fn command(program: &str) -> Command {
    let mut cmd = Command::new(tool(program));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    cmd
}
static AVAILABLE: OnceLock<bool> = OnceLock::new();
/// Starts the ffmpeg/ffprobe check on a background thread (main calls this at startup).
pub fn start_probe() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = thread::Builder::new()
            .name("ffmpeg-probe".into())
            .spawn(tools_present);
    });
}
/// Never blocks (safe on the render thread): None until the background check has finished.
pub fn available() -> Option<bool> {
    if let Some(known) = AVAILABLE.get() {
        return Some(*known);
    }
    start_probe();
    None
}
/// Blocks up to a few seconds the first time (spawns ffmpeg/ffprobe -version); worker threads only.
fn tools_present() -> bool {
    *AVAILABLE.get_or_init(|| {
        ["ffmpeg", "ffprobe"].iter().all(|name| {
            let Ok(mut child) = command(name).arg("-version").stdout(Stdio::null()).spawn() else {
                return false;
            };
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return status.success(),
                    Ok(None) if start.elapsed() < Duration::from_secs(3) => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return false;
                    }
                }
            }
        })
    })
}
#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Zip { archive: PathBuf, entry: String },
}
#[derive(Clone, Debug)]
pub struct Clip {
    pub source: Source,
    pub offset: f64,
}
/// Decode settings, applied in the ffmpeg filter chain (cheap: no GPU pass).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Output fits inside height*16/9 x height (never upscaled).
    pub height: u32,
    /// Frame rate cap; None = the source rate.
    pub fps: Option<f64>,
    /// 0 off, 1 light, 2 heavy box blur.
    pub blur: u8,
}
impl Default for Options {
    fn default() -> Self {
        Options {
            height: 540,
            fps: Some(30.0),
            blur: 0,
        }
    }
}
/// Seconds a video takes to fade in the first time a frame is ready.
const FADE_IN: f64 = 0.4;
/// Highest rate kept for `fps: None`, so a 240 fps screen recording cannot flood the pipe.
const SOURCE_FPS_MAX: f64 = 120.0;
fn filter_chain(info: &Info, blur: u8) -> String {
    // fps first so the scale and blur only run on frames that are kept.
    let mut chain = format!("fps={},scale={}:{}", info.fps, info.width, info.height);
    if blur > 0 {
        let radius = (f64::from(info.height) / if blur == 1 { 90.0 } else { 30.0 }).round() as u32;
        // boxblur refuses a radius over half the smaller side.
        let radius = radius
            .min(u32::from(info.width.min(info.height)) / 2)
            .max(1);
        chain += &format!(",boxblur={radius}:{}", if blur == 1 { 1 } else { 2 });
    }
    chain
}
#[derive(Clone, Copy, Debug)]
struct Info {
    width: u16,
    height: u16,
    fps: f64,
    duration: f64,
}
#[derive(Default)]
struct Control {
    time: f64,
    generation: u64,
    stop: bool,
}
#[derive(Default)]
struct Shared {
    control: Control,
    info: Option<Info>,
    error: Option<String>,
}
struct Frame {
    time: f64,
    generation: u64,
    bytes: Vec<u8>,
}

/// Pure media clock/seek policy. Negative time stays hidden rather than wrapping.
fn timing(t: f64, previous: Option<f64>, duration: f64, looping: bool) -> (f64, bool) {
    let t = if !t.is_finite() {
        -1.0
    } else if looping && t >= 0.0 && duration.is_finite() && duration > 0.0 {
        t.rem_euclid(duration)
    } else {
        t
    };
    let seek = t >= 0.0 && previous.map_or(t > 1.0, |p| p < 0.0 || t < p || t - p > 1.0);
    (t, seek)
}
fn fit(width: u32, height: u32, box_height: u32) -> (u16, u16) {
    let box_height = f64::from(box_height.max(2));
    let scale = (box_height * 16.0 / 9.0 / width as f64)
        .min(box_height / height as f64)
        .min(1.0);
    (
        ((width as f64 * scale) as u16 / 2 * 2).max(2),
        ((height as f64 * scale) as u16 / 2 * 2).max(2),
    )
}
pub struct Video {
    shared: Arc<Mutex<Shared>>,
    child: Arc<Mutex<Option<Child>>>,
    frames: Receiver<Frame>,
    worker: Option<thread::JoinHandle<()>>,
    texture: Option<Texture2D>,
    pending: Option<Frame>,
    previous: Option<f64>,
    looping: bool,
    visible: bool,
    options: Options,
    /// When the first frame went on screen (for the fade-in); Some(-inf) = no fade.
    shown_at: Option<f64>,
}
impl Video {
    pub fn open(clip: &Clip, looping: bool, options: Options) -> Video {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let child = Arc::new(Mutex::new(None));
        let (tx, frames) = mpsc::sync_channel(3);
        let (s, c, source) = (shared.clone(), child.clone(), clip.source.clone());
        let worker = thread::Builder::new()
            .name("video".into())
            .spawn(move || {
                if let Err(error) = decode(source, looping, options, &s, &c, tx) {
                    s.lock().unwrap().error = Some(error);
                }
                kill(&c);
            })
            .inspect_err(|e| shared.lock().unwrap().error = Some(format!("video thread: {e}")))
            .ok();
        Video {
            shared,
            child,
            frames,
            worker,
            texture: None,
            pending: None,
            previous: None,
            looping,
            visible: false,
            options,
            shown_at: None,
        }
    }
    /// The decode settings this video was opened with.
    pub fn options(&self) -> Options {
        self.options
    }
    /// Skips the fade-in (a reopen at new settings replaces a video already on screen).
    pub fn without_fade(mut self) -> Video {
        self.shown_at = Some(f64::NEG_INFINITY);
        self
    }
    pub fn sync(&mut self, t: f64) {
        let (t, generation, info, seek) = {
            let mut s = self.shared.lock().unwrap();
            let (t, seek) = timing(
                t,
                self.previous,
                s.info.map_or(0.0, |i| i.duration),
                self.looping,
            );
            if seek {
                s.control.generation += 1;
            }
            s.control.time = t;
            (t, s.control.generation, s.info, seek)
        };
        self.previous = Some(t);
        if seek {
            self.pending = None;
            self.visible = false;
        }
        if t < 0.0 {
            self.visible = false;
            return;
        }
        let Some(info) = info else {
            return;
        };
        let mut newest = None;
        while let Some(frame) = self.pending.take().or_else(|| self.frames.try_recv().ok()) {
            if frame.generation != generation {
                continue;
            }
            if frame.time > t + 0.000001 {
                self.pending = Some(frame);
                break;
            }
            newest = Some(frame);
        }
        if let Some(frame) = newest {
            match &self.texture {
                Some(texture) => {
                    texture.update_from_bytes(info.width as u32, info.height as u32, &frame.bytes)
                }
                None => {
                    self.texture =
                        Some(Texture2D::from_rgba8(info.width, info.height, &frame.bytes))
                }
            }
            self.visible = true;
            if self.shown_at.is_none() {
                self.shown_at = Some(get_time());
            }
        }
    }
    /// 0..1 fade-in since the first frame went on screen.
    fn fade(&self) -> f32 {
        self.shown_at.map_or(0.0, |at| {
            ((get_time() - at) / FADE_IN).clamp(0.0, 1.0) as f32
        })
    }
    pub fn ready(&self) -> bool {
        self.visible && self.texture.is_some()
    }
    pub fn error(&self) -> Option<String> {
        self.shared.lock().unwrap().error.clone()
    }
    pub fn draw(&self, screen: Vec2, dim: f32) {
        if !self.ready() {
            return;
        }
        let texture = self.texture.as_ref().unwrap();
        let source = crate::wallpaper::cover_uv(
            (texture.width(), texture.height()),
            (screen.x, screen.y),
            Vec2::ZERO,
        );
        let fade = self.fade();
        draw_texture_ex(
            texture,
            0.0,
            0.0,
            Color::new(1.0, 1.0, 1.0, fade),
            DrawTextureParams {
                dest_size: Some(screen),
                source: Some(source),
                ..Default::default()
            },
        );
        draw_rectangle(
            0.0,
            0.0,
            screen.x,
            screen.y,
            Color::new(0.0, 0.0, 0.0, dim.clamp(0.0, 1.0) * fade),
        );
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        self.shared.lock().unwrap().control.stop = true;
        // Never block the render thread. The worker sees `stop`, exits and reaps its own ffmpeg
        // (it only registers a child after checking `stop` under the same lock), so it is
        // detached rather than joined. Killing now unblocks a worker stuck reading a stalled pipe.
        drop(self.worker.take());
        let child = self.child.clone();
        let spawned = thread::Builder::new()
            .name("video-reap".into())
            .spawn(move || kill(&child));
        if spawned.is_err()
            && let Ok(mut slot) = self.child.try_lock()
            && let Some(process) = slot.as_mut()
        {
            let _ = process.kill();
        }
    }
}
fn kill(child: &Mutex<Option<Child>>) {
    if let Some(mut child) = child.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}
/// Extracted zip videos (osu! .osz) live here; capped at `CACHE_CAP` bytes.
const CACHE_DIR: &str = "videos";
const CACHE_CAP: u64 = 2 * 1024 * 1024 * 1024;
const CACHE_EXT: &str = "cache";

fn hash_of(parts: impl Hash) -> u64 {
    let mut hash = DefaultHasher::new();
    parts.hash(&mut hash);
    hash.finish()
}

/// Cache file name: `<stem>-<archive+entry hash>-<archive version hash>.cache`. Everything up
/// to the second dash is the same for every version of one archive entry, so a re-downloaded
/// map replaces its old extraction instead of piling up.
fn cache_names(archive: &Path, entry: &str) -> (String, String) {
    let stem: String = archive
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '(' | ')'))
        .take(48)
        .collect();
    let version = std::fs::metadata(archive)
        .map(|m| hash_of((m.len(), m.modified().ok())))
        .unwrap_or(0);
    let prefix = format!("{stem}-{:016x}-", hash_of((archive, entry)));
    let name = format!("{prefix}{version:016x}.{CACHE_EXT}");
    (prefix, name)
}

/// Which cache files to delete: older versions of the same entry (`prefix`), then the least
/// recently used files until the rest fit in `cap`. `keep` is never deleted.
fn evictions(
    mut files: Vec<(String, u64, std::time::SystemTime)>,
    prefix: &str,
    keep: &str,
    cap: u64,
) -> Vec<String> {
    let mut gone: Vec<String> = Vec::new();
    files.retain(|(name, _, _)| {
        let stale = name != keep && name.starts_with(prefix);
        if stale {
            gone.push(name.clone());
        }
        !stale
    });
    // Newest first; drop from the back.
    files.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    while total > cap {
        let Some(at) = files.iter().rposition(|f| f.0 != keep) else {
            break;
        };
        let (name, size, _) = files.remove(at);
        total -= size;
        gone.push(name);
    }
    gone
}

/// Removes superseded and least recently used extractions from `dir`.
fn trim_cache(dir: &Path, prefix: &str, keep: &str, cap: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let files = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let meta = e.metadata().ok()?;
            (meta.is_file() && name.ends_with(&format!(".{CACHE_EXT}"))).then(|| {
                (
                    name,
                    meta.len(),
                    meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                )
            })
        })
        .collect();
    for name in evictions(files, prefix, keep, cap) {
        // Fails harmlessly on Windows while another video still has the file open.
        let _ = std::fs::remove_file(dir.join(name));
    }
}

fn source_path(source: Source, stopped: &dyn Fn() -> bool) -> Result<PathBuf, String> {
    match source {
        Source::File(path) => Ok(path),
        Source::Zip { archive, entry } => {
            extract_cached(Path::new(CACHE_DIR), &archive, &entry, CACHE_CAP, stopped)
        }
    }
}

fn extract_cached(
    dir: &Path,
    archive: &Path,
    entry: &str,
    cap: u64,
    stopped: &dyn Fn() -> bool,
) -> Result<PathBuf, String> {
    {
        {
            static EXTRACTION: Mutex<()> = Mutex::new(());
            let _guard = EXTRACTION.lock().unwrap_or_else(|e| e.into_inner());
            if stopped() {
                return Err("Video closed".into());
            }
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let (prefix, name) = cache_names(archive, entry);
            let path = dir.join(&name);
            if path.is_file() {
                // Mark as recently used for the LRU trim.
                if let Ok(f) = std::fs::File::options().append(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                trim_cache(dir, &prefix, &name, cap);
                return Ok(path);
            }
            let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            let mut input = zip.by_name(entry).map_err(|e| e.to_string())?;
            if input.size() > 2 * 1024 * 1024 * 1024 {
                return Err("Video zip entry exceeds 2 GB".into());
            }
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let temp = path.with_extension(format!(
                "{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let result = (|| {
                let mut output = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    if stopped() {
                        return Err("Video closed".to_string());
                    }
                    let n = std::io::Read::read(&mut input, &mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut output, &buf[..n]).map_err(|e| e.to_string())?;
                }
                drop(output);
                std::fs::rename(&temp, &path).map_err(|e| e.to_string())
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temp);
            }
            result?;
            trim_cache(dir, &prefix, &name, cap);
            Ok(path)
        }
    }
}
fn probe(
    path: &Path,
    options: Options,
    shared: &Mutex<Shared>,
    child: &Mutex<Option<Child>>,
) -> Result<Info, String> {
    let mut process = command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate,duration:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffprobe not available: {e}"))?;
    let mut stdout = process.stdout.take().unwrap();
    *child.lock().unwrap() = Some(process);
    let start = Instant::now();
    loop {
        if shared.lock().unwrap().control.stop {
            return Err("Video closed".into());
        }
        let status = child
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|p| p.try_wait().ok())
            .flatten();
        if let Some(status) = status {
            kill(child);
            if !status.success() {
                return Err("ffprobe could not read video".into());
            }
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            kill(child);
            return Err("ffprobe took too long".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let mut json = String::new();
    stdout
        .read_to_string(&mut json)
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let stream = json["streams"]
        .as_array()
        .and_then(|s| s.first())
        .ok_or("No video stream in this file")?;
    let width = stream["width"].as_u64().unwrap_or(0) as u32;
    let height = stream["height"].as_u64().unwrap_or(0) as u32;
    if width == 0 || height == 0 {
        return Err("Invalid video dimensions".into());
    }
    let (width, height) = fit(width, height, options.height);
    let rate = stream["avg_frame_rate"].as_str().unwrap_or("30/1");
    let fps = rate
        .split_once('/')
        .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(30.0)
        .min(options.fps.unwrap_or(SOURCE_FPS_MAX));
    let duration = stream["duration"]
        .as_str()
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| {
            json["format"]["duration"]
                .as_str()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0.0);
    Ok(Info {
        width,
        height,
        fps,
        duration,
    })
}
fn decode(
    source: Source,
    looping: bool,
    options: Options,
    shared: &Mutex<Shared>,
    child: &Mutex<Option<Child>>,
    tx: SyncSender<Frame>,
) -> Result<(), String> {
    if !tools_present() {
        return Err(
            "Video requires ffmpeg and ffprobe, on PATH or next to the game (not available)".into(),
        );
    }
    if shared.lock().unwrap().control.stop {
        return Ok(());
    }
    let path = source_path(source, &|| shared.lock().unwrap().control.stop)?;
    if shared.lock().unwrap().control.stop {
        return Ok(());
    }
    let info = probe(&path, options, shared, child)?;
    shared.lock().unwrap().info = Some(info);
    let mut generation = u64::MAX;
    let (mut stdout, mut start, mut index, mut eof) = (None, 0.0, 0_u64, false);
    loop {
        let (stop, current, requested) = {
            let s = shared.lock().unwrap();
            (s.control.stop, s.control.generation, s.control.time)
        };
        if stop {
            return Ok(());
        }
        if generation != current {
            kill(child);
            generation = current;
            start = timing(requested.max(0.0), None, info.duration, looping).0;
            index = 0;
            eof = false;
            let filter = filter_chain(&info, options.blur);
            let mut process = command("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-ss",
                    &format!("{start:.6}"),
                    "-noautorotate",
                    "-threads",
                    "1",
                    "-i",
                ])
                .arg(&path)
                .args([
                    "-an",
                    "-sn",
                    "-dn",
                    "-filter_threads",
                    "1",
                    "-vf",
                    &filter,
                    "-threads",
                    "1",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "pipe:1",
                ])
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| format!("ffmpeg not available: {e}"))?;
            stdout = process.stdout.take();
            let s = shared.lock().unwrap();
            if s.control.stop {
                let _ = process.kill();
                let _ = process.wait();
                return Ok(());
            }
            *child.lock().unwrap() = Some(process);
            drop(s);
        }
        if eof || requested < 0.0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let mut bytes = vec![0; info.width as usize * info.height as usize * 4];
        if let Err(error) = stdout.as_mut().unwrap().read_exact(&mut bytes) {
            let s = shared.lock().unwrap();
            if s.control.stop {
                return Ok(());
            }
            if s.control.generation != generation {
                continue;
            }
            drop(s);
            let failed = child
                .lock()
                .unwrap()
                .as_mut()
                .and_then(|p| p.try_wait().ok())
                .flatten()
                .is_some_and(|s| !s.success());
            kill(child);
            if failed || index == 0 {
                return Err(format!("ffmpeg could not decode video: {error}"));
            }
            eof = true;
            continue;
        }
        let mut frame = Frame {
            time: start + index as f64 / info.fps,
            generation,
            bytes,
        };
        index += 1;
        loop {
            {
                let s = shared.lock().unwrap();
                if s.control.stop {
                    return Ok(());
                }
                if s.control.generation != generation {
                    break;
                }
            }
            match tx.try_send(frame) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return Ok(()),
                Err(TrySendError::Full(f)) => {
                    frame = f;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_tool_next_to_the_game_wins_over_path() {
        let root = std::env::temp_dir().join(format!("rhythm-tool-{}", std::process::id()));
        let (empty, beside) = (root.join("empty"), root.join("beside"));
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&beside).unwrap();
        let file = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
        assert_eq!(tool_in("ffmpeg", [empty.clone()]), PathBuf::from("ffmpeg"));
        std::fs::write(beside.join(&file), b"").unwrap();
        assert_eq!(
            tool_in("ffmpeg", [empty, beside.clone()]),
            beside.join(&file)
        );
        std::fs::create_dir_all(root.join("dir").join(&file)).unwrap();
        assert_eq!(
            tool_in("ffmpeg", [root.join("dir")]),
            PathBuf::from("ffmpeg")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn video_extensions() {
        for name in ["a.MP4", "a.m4v", "a.webm", "a.mkv", "a.mov", "a.AVI"] {
            assert!(is_video(Path::new(name)));
        }
        for name in ["a.mp3", "a.mp4.txt", "a", "a.png"] {
            assert!(!is_video(Path::new(name)));
        }
    }
    #[test]
    fn wrapping_and_seek_decisions() {
        assert_eq!(timing(12.25, Some(2.2), 5.0, true), (2.25, false));
        let (t, seek) = timing(5.1, Some(4.9), 5.0, true);
        assert!((t - 0.1).abs() < 1e-9 && seek);
        assert_eq!(timing(2.0, Some(2.0), 5.0, false), (2.0, false));
        assert!(timing(1.9, Some(2.0), 0.0, false).1);
        assert!(timing(3.1, Some(2.0), 0.0, false).1);
        assert!(!timing(3.0, Some(2.0), 0.0, false).1);
        assert_eq!(timing(-1.0, Some(2.0), 5.0, true), (-1.0, false));
    }
    #[test]
    fn available_never_blocks() {
        let started = Instant::now();
        let _ = available();
        let _ = available();
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        // The background probe settles to the same answer the blocking check gives.
        let known = tools_present();
        assert_eq!(available(), Some(known));
    }
    #[test]
    fn cache_eviction_drops_old_versions_then_least_recently_used() {
        let t = |s: u64| std::time::UNIX_EPOCH + Duration::from_secs(s);
        let gb = 1024 * 1024 * 1024;
        let files = vec![
            ("song-aaaa-v1.cache".to_string(), gb, t(10)),
            ("song-aaaa-v2.cache".to_string(), gb, t(5)),
            ("old-bbbb-v1.cache".to_string(), gb, t(1)),
            ("mid-cccc-v1.cache".to_string(), gb, t(3)),
            ("new-dddd-v1.cache".to_string(), gb / 2, t(20)),
        ];
        let mut gone = evictions(files.clone(), "song-aaaa-", "song-aaaa-v2.cache", 2 * gb);
        gone.sort();
        // v1 superseded; then 3.5 GB left: drop oldest (old) then mid -> 1.5 GB.
        assert_eq!(
            gone,
            [
                "mid-cccc-v1.cache",
                "old-bbbb-v1.cache",
                "song-aaaa-v1.cache"
            ]
        );
        // The kept file survives even when it alone is over the cap.
        let gone = evictions(files, "zzz-", "song-aaaa-v1.cache", 0);
        assert_eq!(gone.len(), 4);
        assert!(!gone.contains(&"song-aaaa-v1.cache".to_string()));
    }
    #[test]
    fn zip_cache_replaces_superseded_extractions() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rhythm-vcache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("map.osz");
        let write = |body: &[u8]| {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            zip.start_file("bg/clip.mp4", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
            zip.finish().unwrap();
        };
        let cache = dir.join("videos");
        write(b"first version");
        let a = extract_cached(&cache, &archive, "bg/clip.mp4", CACHE_CAP, &|| false).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"first version");
        assert_eq!(
            extract_cached(&cache, &archive, "bg/clip.mp4", CACHE_CAP, &|| false).unwrap(),
            a
        );
        write(b"second, longer version");
        let b = extract_cached(&cache, &archive, "bg/clip.mp4", CACHE_CAP, &|| false).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&b).unwrap(), b"second, longer version");
        assert!(!a.exists(), "the superseded extraction is removed");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_closed_video_never_extracts() {
        let dir = std::env::temp_dir().join(format!("rhythm-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("map.osz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("clip.mp4", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, &[7u8; 4096]).unwrap();
        zip.finish().unwrap();
        let cache = dir.join("videos");
        assert!(extract_cached(&cache, &archive, "clip.mp4", CACHE_CAP, &|| true).is_err());
        let left = std::fs::read_dir(&cache).map_or(0, |d| d.count());
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn output_fit_and_video_cover() {
        assert_eq!(fit(1280, 720, 540), (960, 540));
        assert_eq!(fit(720, 1280, 540), (302, 540));
        assert_eq!(fit(1920, 1080, 720), (1280, 720));
        assert_eq!(fit(1920, 1080, 1080), (1920, 1080));
        assert_eq!(fit(1280, 720, 1080), (1280, 720), "never upscaled");
    }
    #[test]
    fn filter_chain_follows_fps_and_blur() {
        let info = Info {
            width: 960,
            height: 540,
            fps: 24.0,
            duration: 1.0,
        };
        assert_eq!(filter_chain(&info, 0), "fps=24,scale=960:540");
        assert_eq!(filter_chain(&info, 1), "fps=24,scale=960:540,boxblur=6:1");
        assert_eq!(filter_chain(&info, 2), "fps=24,scale=960:540,boxblur=18:2");
        let tiny = Info {
            width: 8,
            height: 4,
            ..info
        };
        assert_eq!(filter_chain(&tiny, 2), "fps=24,scale=8:4,boxblur=1:2");
        let crop = crate::wallpaper::cover_uv((960.0, 540.0), (800.0, 600.0), Vec2::ZERO);
        assert_eq!((crop.x, crop.y, crop.w, crop.h), (120.0, 0.0, 720.0, 540.0));
    }
    /// Requires /tmp/t.mp4 generated from the task's 1280x720@60 testsrc + sine command.
    /// Exercises only CPU decode; GPU upload needs a macroquad graphics context.
    #[test]
    #[ignore]
    fn real_decoder() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let child = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel(3);
        let (s, c) = (shared.clone(), child.clone());
        let start = Instant::now();
        let worker = thread::spawn(move || {
            decode(
                Source::File(PathBuf::from("/tmp/t.mp4")),
                false,
                Options::default(),
                &s,
                &c,
                tx,
            )
        });
        let first = rx.recv_timeout(Duration::from_secs(15)).unwrap();
        let first_ms = start.elapsed().as_secs_f64() * 1000.0;
        let info = shared.lock().unwrap().info.unwrap();
        assert_eq!((info.width, info.height, info.fps), (960, 540, 30.0));
        assert_eq!(first.bytes.len(), 960 * 540 * 4);
        let cost = Instant::now();
        for _ in 0..29 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let frame_ms = cost.elapsed().as_secs_f64() * 1000.0 / 29.0;
        {
            let mut s = shared.lock().unwrap();
            s.control.time = 3.0;
            s.control.generation += 1;
        }
        let seek_start = Instant::now();
        let seek = loop {
            let f = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if f.generation == 1 {
                break f;
            }
        };
        assert_eq!(seek.time, 3.0);
        assert_eq!(seek.bytes.len(), 960 * 540 * 4);
        assert_ne!(first.bytes, seek.bytes);
        let forward_ms = seek_start.elapsed().as_secs_f64() * 1000.0;
        let backward_start = Instant::now();
        {
            let mut s = shared.lock().unwrap();
            s.control.time = 0.5;
            s.control.generation += 1;
        }
        let back = loop {
            let f = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if f.generation == 2 {
                break f;
            }
        };
        assert_eq!(back.time, 0.5);
        assert_ne!(first.bytes, back.bytes);
        println!(
            "first_frame={first_ms:.2}ms decode_frame={frame_ms:.3}ms forward_seek={forward_ms:.2}ms backward_seek={:.2}ms size={}x{} fps={}",
            backward_start.elapsed().as_secs_f64() * 1000.0,
            info.width,
            info.height,
            info.fps
        );
        shared.lock().unwrap().control.stop = true;
        kill(&child);
        assert!(worker.join().unwrap().is_ok());
        let audio = crate::audio::decode(std::fs::read("/tmp/t.mp4").unwrap()).unwrap();
        assert!(audio.duration().as_secs_f64() >= 4.9);
        println!("video audio decode: {:.2}s", audio.duration().as_secs_f64());
    }
    /// Requires /tmp/t.mp4 (as `real_decoder`). Dropping a decoding video returns at once and
    /// the killed ffmpeg is still reaped (no zombie left behind).
    #[test]
    #[ignore]
    fn drop_does_not_block_and_reaps_ffmpeg() {
        let clip = Clip {
            source: Source::File(PathBuf::from("/tmp/t.mp4")),
            offset: 0.0,
        };
        let mut worst = Duration::ZERO;
        for _ in 0..5 {
            let video = Video::open(&clip, true, Options::default());
            video.frames.recv_timeout(Duration::from_secs(15)).unwrap();
            let pid = video.child.lock().unwrap().as_ref().unwrap().id();
            let stat = format!("/proc/{pid}/stat");
            assert!(Path::new(&stat).exists());
            // The worker holds the child lock while it kills and waits on ffmpeg for a seek;
            // a drop at that moment must not wait for it.
            let (held, (locked, wait)) = (video.child.clone(), mpsc::channel());
            let holder = thread::spawn(move || {
                let _guard = held.lock().unwrap();
                locked.send(()).unwrap();
                thread::sleep(Duration::from_millis(200));
            });
            wait.recv().unwrap();
            let started = Instant::now();
            drop(video);
            worst = worst.max(started.elapsed());
            holder.join().unwrap();
            // Gone from the process table (a zombie keeps /proc/<pid> with state Z).
            let reaped = Instant::now();
            while Path::new(&stat).exists() {
                assert!(
                    reaped.elapsed() < Duration::from_secs(5),
                    "ffmpeg {pid} not reaped: {:?}",
                    std::fs::read_to_string(&stat)
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        println!("worst drop: {:.3} ms", worst.as_secs_f64() * 1000.0);
        assert!(worst < Duration::from_micros(500), "drop took {worst:?}");
    }
}
