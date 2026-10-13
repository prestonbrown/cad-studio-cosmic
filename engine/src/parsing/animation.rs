//! animation.json — motion study decoding and time interpolation, faithful
//! to upstream `MotionStudy.swift`: every frame field except `t` is
//! best-effort (upstream wraps each decode in `try? ?? default`), frames
//! sort by `t` on load, wrong-arity `translate`/`rotate` decode as
//! zero/identity, and non-unit quaternions normalize on load.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

/// A part's placement at one keyframe. Both fields are load-time
/// normalized: `translate` of any arity but 3 decodes as zero (upstream
/// does not pad), `rotate` of any arity but 4 decodes as identity — and an
/// array holding any non-number decodes as zero/identity too, because
/// upstream's `[Float]` decode is all-or-nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pose {
    pub translate: [f32; 3],
    pub rotate: glam::Quat,
}

impl Pose {
    /// Upstream `simd_mix`/`simd_slerp`: linear blend of translates,
    /// shortest-arc blend of rotations.
    fn interpolated(self, other: Pose, fraction: f32) -> Pose {
        Pose {
            translate: [
                self.translate[0] + (other.translate[0] - self.translate[0]) * fraction,
                self.translate[1] + (other.translate[1] - self.translate[1]) * fraction,
                self.translate[2] + (other.translate[2] - self.translate[2]) * fraction,
            ],
            rotate: self.rotate.slerp(other.rotate, fraction),
        }
    }
}

impl<'de> Deserialize<'de> for Pose {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(de)?;
        // Upstream decodes [Float] all-or-nothing (try? ?? []): one
        // non-number entry drops the whole array before the arity gate.
        let nums = |key: &str| -> Option<Vec<f32>> {
            v.get(key).and_then(|v| v.as_array()).and_then(|a| {
                a.iter()
                    .map(|x| x.as_f64().map(|n| n as f32))
                    .collect::<Option<Vec<_>>>()
            })
        };
        let t = nums("translate");
        let r = nums("rotate");
        Ok(Pose {
            translate: match t.as_deref() {
                Some([x, y, z]) => [*x, *y, *z],
                _ => [0.0; 3],
            },
            rotate: match r.as_deref() {
                Some([x, y, z, w]) => glam::Quat::from_xyzw(*x, *y, *z, *w).normalize(),
                _ => glam::Quat::IDENTITY,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Collision {
    pub parts: Vec<String>,
    /// Intersection volume in mm³ when the agent reported one.
    pub volume: Option<f64>,
}

/// One keyframe. `values` and `collisions` decode with per-entry tolerance
/// — malformed entries drop individually, where upstream drops the whole
/// container (`try? ?? default`); a documented widening.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    pub t: f64,
    pub label: Option<String>,
    pub parts: HashMap<String, Pose>,
    pub values: BTreeMap<String, f64>,
    pub collisions: Vec<Collision>,
}

impl<'de> Deserialize<'de> for Frame {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(de)?;
        let t = v
            .get("t")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| serde::de::Error::custom("frame field t: expected a number"))?;
        let parts = match v.get("parts").and_then(|v| v.as_object()) {
            Some(m) => m
                .iter()
                .map(|(k, pv)| Pose::deserialize(pv).map(|p| (k.clone(), p)))
                .collect::<Result<HashMap<_, _>, _>>()
                .map_err(serde::de::Error::custom)?,
            None => HashMap::new(),
        };
        let values = v
            .get("values")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n)))
                    .collect()
            })
            .unwrap_or_default();
        let collisions = v
            .get("collisions")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let parts = e
                            .get("parts")?
                            .as_array()?
                            .iter()
                            .filter_map(|p| p.as_str().map(String::from))
                            .collect::<Vec<_>>();
                        let volume = e.get("volume").and_then(|v| v.as_f64());
                        Some(Collision { parts, volume })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Frame {
            t,
            label: v.get("label").and_then(|v| v.as_str()).map(String::from),
            parts,
            values,
            collisions,
        })
    }
}

/// The study's state at one instant: what M7 renders.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StudyState {
    pub parts: HashMap<String, Pose>,
    /// Display order comes from [`MotionStudy::value_names`] (first-seen,
    /// then sorted — upstream's `State.values`), which iterates this map by
    /// name; iterating the map directly would re-sort by key instead, so
    /// players must go through `value_names()`.
    pub values: BTreeMap<String, f64>,
    pub label: Option<String>,
    pub collisions: Vec<Collision>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MotionStudy {
    pub title: Option<String>,
    pub frames: Vec<Frame>,
    /// Union of per-frame `values` keys, ordered by first frame appearance
    /// then sorted within a frame — upstream `valueNames`.
    value_names: Vec<String>,
}

impl MotionStudy {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }

    /// Upstream `duration`: the last frame's time (frames are sorted on load).
    pub fn duration(&self) -> f64 {
        self.frames.last().map_or(0.0, |f| f.t)
    }

    /// A study animates when it holds more than one keyframe; a single
    /// frame is a snapshot, shown without motion controls.
    pub fn is_active(&self) -> bool {
        self.frames.len() > 1
    }

    pub fn value_names(&self) -> Vec<&str> {
        self.value_names.iter().map(String::as_str).collect()
    }

    pub fn state_at(&self, time: f64) -> StudyState {
        let Some(first) = self.frames.first() else {
            return StudyState::default();
        };
        // First frame at or after `time`; before the first keyframe and past
        // the last, the surrounding span clamps the fraction instead.
        let upper = self
            .frames
            .iter()
            .position(|f| f.t >= time)
            .unwrap_or(self.frames.len() - 1);
        let next = &self.frames[upper];
        let previous = if upper > 0 {
            &self.frames[upper - 1]
        } else {
            first
        };
        let span = next.t - previous.t;
        let fraction = if span > 0.0 {
            ((time - previous.t) / span).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };

        let mut parts = HashMap::new();
        for name in previous.parts.keys().chain(next.parts.keys()) {
            // A part missing from one surrounding frame holds the other's
            // pose across the whole span (upstream falls back both ways).
            let start = previous
                .parts
                .get(name)
                .or_else(|| next.parts.get(name))
                .copied()
                .unwrap_or_default();
            let end = next.parts.get(name).copied().unwrap_or(start);
            parts.insert(name.clone(), start.interpolated(end, fraction));
        }

        let mut values = BTreeMap::new();
        for name in &self.value_names {
            let Some(start) = previous.values.get(name).or_else(|| next.values.get(name)) else {
                continue;
            };
            let end = next.values.get(name).copied().unwrap_or(*start);
            values.insert(name.clone(), start + (end - start) * fraction as f64);
        }

        // Collisions come from the nearest frame; an exact tie (fraction
        // 0.5) goes to the later one, as upstream's `< 0.5` does.
        let nearest = if fraction < 0.5 { previous } else { next };
        // An unlabeled frame does not clear an earlier label: the label is
        // the last frame at or before `time` that carries one.
        let label = self
            .frames
            .iter()
            .rev()
            .find(|f| f.t <= time && f.label.is_some())
            .and_then(|f| f.label.clone());
        StudyState {
            parts,
            values,
            label,
            collisions: nearest.collisions.clone(),
        }
    }
}

impl<'de> Deserialize<'de> for MotionStudy {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(de)?;
        let obj = v
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("motion study: expected a JSON object"))?;
        let frames_json = obj
            .get("frames")
            .and_then(|v| v.as_array())
            .ok_or_else(|| serde::de::Error::custom("motion study: frames must be an array"))?;
        let mut frames: Vec<Frame> = frames_json
            .iter()
            .map(Frame::deserialize)
            .collect::<Result<_, _>>()
            .map_err(serde::de::Error::custom)?;
        frames.sort_by(|a, b| a.t.total_cmp(&b.t));
        let mut value_names: Vec<String> = Vec::new();
        for frame in &frames {
            for name in frame.values.keys() {
                if !value_names.iter().any(|n| n == name) {
                    value_names.push(name.clone());
                }
            }
        }
        Ok(MotionStudy {
            title: obj.get("title").and_then(|v| v.as_str()).map(String::from),
            frames,
            value_names,
        })
    }
}
