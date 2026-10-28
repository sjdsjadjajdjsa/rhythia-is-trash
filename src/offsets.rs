//! Per-song music video offsets (`,` / `.` in game), kept next to scores.json.
use std::{collections::BTreeMap, fs};

use serde_json::{Map, Value};

const FILE: &str = "video_offsets.json";
/// One `,` / `.` press.
pub const STEP_MS: f64 = 25.0;
/// Further than this a video is the wrong video, not a late one.
const LIMIT_MS: f64 = 10_000.0;

#[derive(Default)]
pub struct VideoOffsets {
    ms: BTreeMap<String, f64>,
}

fn parse(text: &str) -> Option<BTreeMap<String, f64>> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    Some(
        map.into_iter()
            .filter_map(|(key, v)| Some((key, v.as_f64()?.clamp(-LIMIT_MS, LIMIT_MS))))
            .collect(),
    )
}

fn render(ms: &BTreeMap<String, f64>) -> String {
    let map: Map<String, Value> = ms
        .iter()
        .filter(|(_, v)| **v != 0.0)
        .map(|(key, v)| (key.clone(), Value::from(*v)))
        .collect();
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default()
}

impl VideoOffsets {
    pub fn load() -> Self {
        let Ok(text) = fs::read_to_string(FILE) else {
            return Self::default();
        };
        let Some(ms) = parse(&text) else {
            eprintln!("{FILE}: not valid, ignoring it");
            return Self::default();
        };
        Self { ms }
    }

    /// Milliseconds, positive = the video shows later.
    pub fn get(&self, key: &str) -> f64 {
        self.ms.get(key).copied().unwrap_or(0.0)
    }

    /// Moves the offset by `steps` presses and returns the new value.
    pub fn nudge(&mut self, key: &str, steps: i32) -> f64 {
        let v = (self.get(key) + f64::from(steps) * STEP_MS).clamp(-LIMIT_MS, LIMIT_MS);
        if v == 0.0 {
            self.ms.remove(key);
        } else {
            self.ms.insert(key.into(), v);
        }
        v
    }

    pub fn save(&self) {
        if let Err(e) = crate::store::write_atomic(FILE, &render(&self.ms)) {
            eprintln!("{FILE}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_round_trip_and_ignore_junk() {
        let mut o = VideoOffsets::default();
        assert_eq!(o.nudge("a.osz", 1), 25.0);
        assert_eq!(o.nudge("a.osz", 1), 50.0);
        assert_eq!(o.nudge("b.mp4", -3), -75.0);
        assert_eq!(o.nudge("c.mp4", 1), 25.0);
        assert_eq!(o.nudge("c.mp4", -1), 0.0);
        let again = VideoOffsets {
            ms: parse(&render(&o.ms)).unwrap(),
        };
        assert_eq!((again.get("a.osz"), again.get("b.mp4")), (50.0, -75.0));
        assert_eq!(again.get("c.mp4"), 0.0);
        assert!(
            !render(&o.ms).contains("c.mp4"),
            "zero offsets are not stored"
        );
        assert!(parse("not json").is_none());
        let wild = parse(r#"{"x": "bad", "y": 1e9}"#).unwrap();
        assert_eq!((wild.get("x"), wild.get("y")), (None, Some(&LIMIT_MS)));
    }
}
