//! Ghost races: the best run on each song (or a friend's, dropped onto the window) replays as a
//! second cursor with its score, so a run can be raced live. Files live in `ghosts/`, friends'
//! in `ghosts/friends/`.
use std::{
    fs,
    path::{Path, PathBuf},
};

use macroquad::prelude::vec2;

use crate::stats::Replay;

const DIR: &str = "ghosts";
const FRIENDS: &str = "ghosts/friends";
const MAGIC: &[u8; 8] = b"RGHOST1\n";
const MAX_BYTES: u64 = 64 << 20;
const MAX_TEXT: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    pub key: String,
    pub name: String,
    pub score: u64,
    pub accuracy: f64,
    pub path: Replay,
    /// (song time, total score) after every hit, in time order.
    pub scores: Vec<(f32, u64)>,
}

impl Ghost {
    pub fn score_at(&self, t: f64) -> u64 {
        let i = self.scores.partition_point(|(at, _)| f64::from(*at) <= t);
        i.checked_sub(1).map_or(0, |i| self.scores[i].1)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for text in [&self.key, &self.name] {
            let text = truncate(text);
            out.extend((text.len() as u16).to_le_bytes());
            out.extend(text.as_bytes());
        }
        out.extend(self.score.to_le_bytes());
        out.extend(self.accuracy.to_le_bytes());
        let samples = self.path.samples();
        out.extend((samples.len() as u32).to_le_bytes());
        for &(t, p) in samples {
            for v in [t, p.x, p.y] {
                out.extend(v.to_le_bytes());
            }
        }
        out.extend((self.scores.len() as u32).to_le_bytes());
        for &(t, s) in &self.scores {
            out.extend(t.to_le_bytes());
            out.extend(s.to_le_bytes());
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader(bytes.strip_prefix(MAGIC)?);
        let key = r.text()?;
        let name = r.text()?;
        let score = u64::from_le_bytes(r.array()?);
        let accuracy = f64::from_le_bytes(r.array()?);
        let n = r.count(12)?;
        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t = f32::from_le_bytes(r.array()?);
            let x = f32::from_le_bytes(r.array()?);
            let y = f32::from_le_bytes(r.array()?);
            samples.push((t, vec2(x, y)));
        }
        let m = r.count(12)?;
        let mut scores: Vec<(f32, u64)> = Vec::with_capacity(m);
        for _ in 0..m {
            let t = f32::from_le_bytes(r.array()?);
            let s = u64::from_le_bytes(r.array()?);
            if !t.is_finite() || scores.last().is_some_and(|&(lt, ls)| t < lt || s < ls) {
                return None;
            }
            scores.push((t, s));
        }
        let path = Replay::from_samples(&samples);
        if !r.0.is_empty() || path.is_empty() || !accuracy.is_finite() {
            return None;
        }
        Some(Self {
            key,
            name,
            score,
            accuracy,
            path,
            scores,
        })
    }

    pub fn save(&self) {
        let path = own_file(&self.key);
        let result = fs::create_dir_all(DIR)
            .and_then(|()| crate::store::write_atomic(&path.to_string_lossy(), self.encode()));
        if let Err(e) = result {
            eprintln!("{}: {e}", path.display());
        }
    }
}

pub fn is_ghost(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ghost"))
}

/// The strongest ghost for this song and difficulty: your own best or any friend's.
pub fn best(key: &str) -> Option<Ghost> {
    best_in(Path::new(DIR), Path::new(FRIENDS), key)
}

fn best_in(dir: &Path, friends: &Path, key: &str) -> Option<Ghost> {
    let own = read(&dir.join(file_name(key))).map(|g| Ghost {
        name: "your best".into(),
        ..g
    });
    let others = fs::read_dir(friends)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_ghost(p))
        .filter_map(|p| read(&p));
    own.into_iter()
        .chain(others)
        .filter(|g| g.key == key)
        .max_by_key(|g| g.score)
}

/// Copies a dropped ghost file into the friends folder; returns a line for the song list.
pub fn import(src: &Path) -> Result<String, String> {
    import_into(Path::new(FRIENDS), src)
}

fn import_into(friends: &Path, src: &Path) -> Result<String, String> {
    let shown = src.file_name().map_or_else(
        || src.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let ghost = read(src).ok_or_else(|| format!("{shown}: not a ghost file"))?;
    fs::create_dir_all(friends).map_err(|e| format!("{shown}: {e}"))?;
    let name = format!("{} - {}", sanitize(&ghost.name), file_name(&ghost.key));
    fs::write(friends.join(name), ghost.encode()).map_err(|e| format!("{shown}: {e}"))?;
    Ok(format!(
        "ghost of {} added for {} (race it there)",
        ghost.name, ghost.key
    ))
}

fn read(path: &Path) -> Option<Ghost> {
    if fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    Ghost::decode(&fs::read(path).ok()?)
}

fn own_file(key: &str) -> PathBuf {
    Path::new(DIR).join(file_name(key))
}

fn file_name(key: &str) -> String {
    format!("{}.ghost", sanitize(key))
}

fn sanitize(s: &str) -> String {
    let clean: String = truncate(s)
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.[]()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    match clean.trim_matches(['.', ' ']) {
        "" => "_".into(),
        s => s.into(),
    }
}

fn truncate(s: &str) -> &str {
    let mut end = s.len().min(MAX_TEXT);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn text(&mut self) -> Option<String> {
        let n = u16::from_le_bytes(self.array()?) as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }

    fn count(&mut self, item: usize) -> Option<usize> {
        let n = u32::from_le_bytes(self.array()?) as usize;
        (n <= self.0.len() / item).then_some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ghost(key: &str, score: u64) -> Ghost {
        let mut path = Replay::new();
        path.record(0.0, vec2(0.0, 0.0));
        path.record(1.0, vec2(0.5, -0.5));
        Ghost {
            key: key.into(),
            name: "ana".into(),
            score,
            accuracy: 97.5,
            path,
            scores: vec![(0.5, 100), (0.9, score)],
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rhythm-ghost-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_and_rejects_damaged_files() {
        let g = ghost("a.sspm [Hard]", 300);
        let bytes = g.encode();
        assert_eq!(Ghost::decode(&bytes), Some(g));
        assert!(Ghost::decode(&bytes[..bytes.len() - 1]).is_none());
        assert!(Ghost::decode(b"RGHOST1\n\xff\xff").is_none());
        assert!(Ghost::decode(b"not a ghost").is_none());
        let mut huge = bytes.clone();
        let at = MAGIC.len() + 2 + 13 + 2 + 3 + 16;
        huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Ghost::decode(&huge).is_none());
    }

    #[test]
    fn score_follows_the_hits_in_time() {
        let g = ghost("a", 300);
        assert_eq!(g.score_at(0.0), 0);
        assert_eq!(g.score_at(0.5), 100);
        assert_eq!(g.score_at(0.7), 100);
        assert_eq!(g.score_at(5.0), 300);
        assert_eq!(g.score_at(f64::NAN), 0);
    }

    #[test]
    fn races_the_strongest_ghost_for_the_same_song_only() {
        let root = temp("best");
        let (dir, friends) = (root.join("ghosts"), root.join("ghosts/friends"));
        fs::create_dir_all(&friends).unwrap();
        assert!(best_in(&dir, &friends, "a").is_none());
        fs::write(dir.join(file_name("a")), ghost("a", 200).encode()).unwrap();
        assert_eq!(best_in(&dir, &friends, "a").unwrap().name, "your best");
        let dropped = root.join("from ana.ghost");
        fs::write(&dropped, ghost("a", 500).encode()).unwrap();
        import_into(&friends, &dropped).unwrap();
        fs::write(&dropped, ghost("b", 900).encode()).unwrap();
        import_into(&friends, &dropped).unwrap();
        let won = best_in(&dir, &friends, "a").unwrap();
        assert_eq!((won.name.as_str(), won.score), ("ana", 500));
        fs::write(&dropped, b"junk").unwrap();
        assert!(import_into(&friends, &dropped).is_err());
        assert_eq!(sanitize("../x/y?.osz"), "_x_y_.osz");
        let _ = fs::remove_dir_all(&root);
    }
}
