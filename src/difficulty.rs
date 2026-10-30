//! Difficulty levels and song speed. Songs charted from their audio are re-picked from the
//! detected onsets and snapped to the beat; hand-made maps are thinned (easier) or spread
//! toward the grid edges (harder), never given notes their mapper did not place.

use std::f64::consts::TAU;

use crate::{
    chart::{self, Onset, Pick, Shape, Song},
    map::{Map, Note},
};

/// Saved names, in order; the index is the level.
pub const LEVELS: [&str; 4] = ["easy", "normal", "hard", "insane"];
pub const NAMES: [&str; 4] = ["Easy", "Normal", "Hard", "Insane"];
pub const NORMAL: usize = 1;
pub const SPEEDS: [&str; 7] = ["0.75", "0.85", "1", "1.15", "1.25", "1.35", "1.5"];
pub const ONE_X: usize = 2;

struct Tune {
    pick: Pick,
    shape: Shape,
    /// How much being on the beat counts when choosing which onsets stay.
    beat: f32,
    /// Grid steps per beat that notes snap to.
    subdiv: f64,
    /// Hand-made maps: shortest gap kept, distance scale and edge spread exponent.
    thin: f64,
    pull: f32,
    spread: f32,
}

const TUNES: [Tune; 4] = [
    Tune {
        pick: Pick {
            gap: 0.3,
            floor: 0.12,
            contrast: 1.5,
        },
        shape: Shape {
            reach: -1,
            far: (0.6, 0.0),
        },
        beat: 0.8,
        subdiv: 2.0,
        thin: 0.3,
        pull: 0.8,
        spread: 1.0,
    },
    Tune {
        pick: Pick {
            gap: 0.2,
            floor: 0.07,
            contrast: 1.25,
        },
        shape: chart::NORMAL_SHAPE,
        beat: 0.3,
        subdiv: 4.0,
        thin: 0.0,
        pull: 1.0,
        spread: 1.0,
    },
    Tune {
        pick: chart::BASE,
        shape: Shape {
            reach: 0,
            far: (0.8, 0.2),
        },
        beat: 0.2,
        subdiv: 4.0,
        thin: 0.0,
        pull: 1.0,
        spread: 0.75,
    },
    Tune {
        pick: Pick {
            gap: 0.07,
            floor: 0.02,
            contrast: 0.8,
        },
        shape: Shape {
            reach: 1,
            far: (0.6, 0.4),
        },
        beat: 0.1,
        subdiv: 4.0,
        thin: 0.0,
        pull: 1.0,
        spread: 0.55,
    },
];

pub fn speed(index: usize) -> f64 {
    SPEEDS[index.min(SPEEDS.len() - 1)].parse().unwrap_or(1.0)
}

/// "Hard 1.25x", or empty for normal at 1x.
pub fn tag(level: usize, speed_index: usize) -> String {
    let mut parts = Vec::new();
    if level != NORMAL {
        parts.push(NAMES[level.min(NAMES.len() - 1)].to_string());
    }
    if speed_index != ONE_X {
        parts.push(format!("{}x", SPEEDS[speed_index.min(SPEEDS.len() - 1)]));
    }
    parts.join(" ")
}

/// Best scores are kept per song, level and speed; normal at 1x keeps the plain name.
pub fn score_key(label: &str, tag: &str) -> String {
    if tag.is_empty() {
        label.to_string()
    } else {
        format!("{label} [{tag}]")
    }
}

/// The notes of `map` at `level`.
pub fn notes(map: &Map, level: usize) -> Vec<Note> {
    let level = level.min(TUNES.len() - 1);
    let notes = match &map.auto {
        Some(song) => from_song(song, level),
        None => from_map(&map.notes, level),
    };
    if notes.is_empty() {
        map.notes.clone()
    } else {
        notes
    }
}

pub fn from_song(song: &Song, level: usize) -> Vec<Note> {
    let tune = &TUNES[level.min(TUNES.len() - 1)];
    let strong: Vec<(f64, f32)> = song
        .onsets
        .iter()
        .filter(|o| o.strength >= chart::BASE.floor && o.contrast > chart::BASE.contrast)
        .map(|o| (o.time, o.strength))
        .collect();
    let grid = Grid::find(&strong);
    let picked = chart::pick(&song.onsets, &tune.pick, |o| {
        o.strength * (1.0 - tune.beat + tune.beat * grid.weight(o.time))
    });
    let mut moved: Vec<Onset> = picked
        .into_iter()
        .map(|o| Onset {
            time: grid.snap(o.time, tune.subdiv),
            ..o
        })
        .collect();
    moved.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut snapped: Vec<Onset> = Vec::with_capacity(moved.len());
    for o in moved {
        match snapped.last_mut() {
            Some(last) if o.time - last.time < 0.001 => {
                if o.strength > last.strength {
                    *last = o;
                }
            }
            _ => snapped.push(o),
        }
    }
    chart::layout(&snapped, song.seed, &tune.shape)
}

fn from_map(notes: &[Note], level: usize) -> Vec<Note> {
    let tune = &TUNES[level];
    let mut out = if tune.thin > 0.0 {
        let grid = Grid::find(&notes.iter().map(|n| (n.time, 1.0)).collect::<Vec<_>>());
        let mut ranked: Vec<(f32, Note)> =
            notes.iter().map(|n| (grid.weight(n.time), *n)).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        chart::spaced(ranked.into_iter().map(|(_, n)| n), tune.thin, |n| n.time)
    } else {
        notes.to_vec()
    };
    let shape = |p: f32| p.signum() * p.abs().powf(tune.spread) * tune.pull;
    for n in &mut out {
        n.x = shape(n.x);
        n.y = shape(n.y);
    }
    out
}

/// A rough 0-10 rating: how many notes per second, weighted by how far each jump is,
/// averaged over the busiest quarter of the song.
pub fn stars(notes: &[Note]) -> f32 {
    const CHUNK: f64 = 2.0;
    if notes.is_empty() {
        return 0.0;
    }
    let mut load: std::collections::HashMap<u64, f64> = std::collections::HashMap::new();
    for pair in notes.windows(2) {
        let gap = (pair[1].time - pair[0].time).max(0.06);
        let dist = f64::from((pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y));
        *load.entry((pair[1].time / CHUNK) as u64).or_default() += (1.0 + 1.2 * dist) / gap;
    }
    let mut busy: Vec<f64> = load.into_values().map(|l| l / CHUNK).collect();
    if busy.is_empty() {
        return 0.0;
    }
    busy.sort_by(|a, b| b.total_cmp(a));
    let top = busy.len().div_ceil(4);
    let mean = busy[..top].iter().sum::<f64>() / top as f64;
    (0.65 * mean.powf(0.4)) as f32
}

/// Stars at a speed: notes come `speed` times as often and the window shrinks with them.
pub fn stars_at(stars: f32, speed_index: usize) -> f32 {
    stars * speed(speed_index).powf(0.8) as f32
}

const WINDOW: f64 = 10.0;
const HOP: f64 = 5.0;
const MIN_PERIOD: f64 = 0.1;
const MAX_PERIOD: f64 = 1.0;
const ON_BEAT: f64 = 0.03;
/// Random times line up to about 1/sqrt(n); a pulse has to beat that several times over.
const SIGNIFICANT: f64 = 3.5;

/// The beat over time: a period and phase per stretch of the song, so tempo drift and
/// tempo changes are followed. Stretches without a steady pulse have none.
pub struct Grid {
    spans: Vec<(f64, f64, f64)>,
}

impl Grid {
    pub fn find(points: &[(f64, f32)]) -> Grid {
        let Some(end) = points.last().map(|p| p.0) else {
            return Grid { spans: Vec::new() };
        };
        let mut spans = Vec::new();
        let mut start = points[0].0;
        loop {
            let lo = points.partition_point(|p| p.0 < start);
            let hi = points.partition_point(|p| p.0 < start + WINDOW);
            if let Some((period, phase)) = fit(&points[lo..hi]) {
                spans.push((start + WINDOW / 2.0, period, phase));
            }
            if start + WINDOW >= end {
                break;
            }
            start += HOP;
            let next = points[points.partition_point(|p| p.0 < start)].0;
            if next > start + WINDOW {
                start = next;
            }
        }
        Grid { spans }
    }

    fn at(&self, t: f64) -> Option<(f64, f64)> {
        self.spans
            .iter()
            .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
            .filter(|s| (s.0 - t).abs() <= WINDOW)
            .map(|s| (s.1, s.2))
    }

    /// 1 on the beat, less on halves and quarters, least off the grid.
    pub fn weight(&self, t: f64) -> f32 {
        let Some((period, phase)) = self.at(t) else {
            return 0.5;
        };
        let near = |sub: f64| {
            let step = period / sub;
            let off = (t - phase).rem_euclid(step);
            off.min(step - off) <= ON_BEAT
        };
        if near(1.0) {
            1.0
        } else if near(2.0) {
            0.6
        } else if near(4.0) {
            0.35
        } else {
            0.15
        }
    }

    /// `t` moved onto the nearest 1/`subdiv` beat when it is close to one.
    pub fn snap(&self, t: f64, subdiv: f64) -> f64 {
        let Some((period, phase)) = self.at(t) else {
            return t;
        };
        let step = period / subdiv;
        let s = phase + ((t - phase) / step).round() * step;
        if (s - t).abs() <= (step * 0.4).min(0.04) {
            s
        } else {
            t
        }
    }
}

fn coherence(points: &[(f64, f32)], period: f64) -> (f64, f64) {
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for &(t, w) in points {
        let a = TAU * t / period;
        let w = f64::from(w);
        re += w * a.cos();
        im += w * a.sin();
        sum += w;
    }
    (re.hypot(im) / sum.max(1e-9), im.atan2(re))
}

/// The longest period that explains the points nearly as well as the best one (any divisor
/// of the beat lines up too), refined to a tenth of a millisecond.
fn fit(all: &[(f64, f32)]) -> Option<(f64, f64)> {
    let mut levels: Vec<f32> = all.iter().map(|p| p.1).collect();
    levels.sort_by(|a, b| b.total_cmp(a));
    let cut = levels.get(levels.len() / 3).copied().unwrap_or(0.0);
    let points: Vec<(f64, f32)> = all.iter().copied().filter(|p| p.1 >= cut).collect();
    let points = points.as_slice();
    if points.len() < 8 {
        return None;
    }
    let steps = ((MAX_PERIOD - MIN_PERIOD) / 0.002) as usize;
    let scores: Vec<(f64, f64)> = (0..=steps)
        .map(|i| {
            let p = MAX_PERIOD - i as f64 * 0.002;
            (p, coherence(points, p).0)
        })
        .collect();
    let best = scores.iter().map(|s| s.1).fold(0.0, f64::max);
    if best * (points.len() as f64).sqrt() < SIGNIFICANT {
        return None;
    }
    let mut i = scores.iter().position(|s| s.1 >= 0.85 * best)?;
    while scores.get(i + 1).is_some_and(|next| next.1 > scores[i].1) {
        i += 1;
    }
    let coarse = scores[i].0;
    let period = (-20..=20)
        .map(|k| coarse + f64::from(k) * 0.0001)
        .max_by(|a, b| coherence(points, *a).0.total_cmp(&coherence(points, *b).0))?;
    let angle = coherence(points, period).1;
    Some((period, angle.rem_euclid(TAU) / TAU * period))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(time: f64, x: f32, y: f32) -> Note {
        Note { time, x, y }
    }

    fn song(times: &[f64]) -> Song {
        Song {
            onsets: times
                .iter()
                .enumerate()
                .map(|(i, &time)| Onset {
                    time,
                    strength: [1.0, 0.3, 0.6, 0.3][i % 4],
                    contrast: 3.0,
                })
                .collect(),
            seed: 3,
        }
    }

    #[test]
    fn finds_the_beat_and_snaps_a_sloppy_onset_onto_it() {
        let beat = 60.0 / 128.0;
        let points: Vec<(f64, f32)> = (0..64).map(|i| (1.0 + f64::from(i) * beat, 1.0)).collect();
        let grid = Grid::find(&points);
        assert!(grid.weight(1.0 + 10.0 * beat) > 0.9);
        assert!(grid.weight(1.0 + 10.5 * beat) < 0.9);
        let sloppy = 1.0 + 20.0 * beat + 0.018;
        assert!((grid.snap(sloppy, 4.0) - (1.0 + 20.0 * beat)).abs() < 0.002);
        let far_off = 1.0 + 20.0 * beat + beat / 8.0;
        assert_eq!(grid.snap(far_off, 2.0), far_off);
    }

    #[test]
    fn no_pulse_means_no_snapping() {
        let points: Vec<(f64, f32)> = (0..40)
            .map(|i| (f64::from(i) * 0.37 + f64::from(i * i % 11) * 0.053, 1.0))
            .collect();
        let grid = Grid::find(&points);
        assert_eq!(grid.snap(3.217, 4.0), 3.217);
    }

    #[test]
    fn harder_songs_have_more_notes_and_more_stars() {
        let times: Vec<f64> = (0..1200).map(|i| 1.0 + f64::from(i) * 0.025).collect();
        let s = song(&times);
        let counts: Vec<usize> = (0..4).map(|l| from_song(&s, l).len()).collect();
        let rated: Vec<f32> = (0..4).map(|l| stars(&from_song(&s, l))).collect();
        assert!(counts.windows(2).all(|w| w[0] < w[1]), "{counts:?}");
        assert!(rated.windows(2).all(|w| w[0] < w[1]), "{rated:?}");
        for l in 0..4 {
            let n = from_song(&s, l);
            assert!(n.windows(2).all(|w| w[1].time > w[0].time));
        }
    }

    #[test]
    fn easy_thins_a_map_and_never_adds_notes() {
        let notes: Vec<Note> = (0..100)
            .map(|i| {
                note(
                    i as f64 * 0.125,
                    [-1.0, 0.0, 1.0][i % 3],
                    [1.0, -1.0][i % 2],
                )
            })
            .collect();
        let easy = from_map(&notes, 0);
        assert!(easy.len() < notes.len() / 2, "{}", easy.len());
        assert!(easy.windows(2).all(|w| w[1].time - w[0].time >= 0.3 - 1e-9));
        assert!(easy.iter().all(|n| n.x.abs() <= 0.8 + 1e-6));
        assert_eq!(from_map(&notes, NORMAL).len(), notes.len());
        let hard = from_map(&notes, 3);
        assert_eq!(hard.len(), notes.len());
        let inner = [note(0.0, 0.4, -0.4)];
        assert!(from_map(&inner, 3)[0].x > 0.4);
    }

    #[test]
    fn a_note_far_in_the_future_costs_nothing() {
        let mut notes: Vec<Note> = (0..20)
            .map(|i| note(f64::from(i) * 0.5, 0.0, 0.0))
            .collect();
        notes.push(note(1e12, 1.0, 1.0));
        assert!(stars(&notes).is_finite());
        let started = std::time::Instant::now();
        for level in 0..4 {
            from_map(&notes, level);
        }
        assert!(started.elapsed().as_secs_f64() < 1.0);
    }

    #[test]
    fn speed_and_tags_name_the_run() {
        assert_eq!(speed(ONE_X), 1.0);
        assert_eq!(tag(NORMAL, ONE_X), "");
        assert_eq!(tag(2, 4), "Hard 1.25x");
        assert_eq!(score_key("a.osz", ""), "a.osz");
        assert_eq!(score_key("a.osz", "Easy"), "a.osz [Easy]");
        assert!(stars_at(3.0, 6) > 3.0 && stars_at(3.0, 0) < 3.0);
    }
}
