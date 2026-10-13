//! Motion playback: the upstream state machine plus the pure derivation
//! that turns one study state into per-part poses and appearances.
//!
//! [`MotionPlayer`] transcribes MotionPlayer.swift verbatim — loop wrap by
//! remainder, clamp-and-stop only when looping is off, a paused player
//! never advances, and play restarts from 0 only when time sits at the
//! duration. The clock that drives it lives in the app; this module is
//! pure state in, uniform-ready slots out (spec §4.3: no iced/cosmic
//! type crosses it).
//!
//! [`poses`] ports ModelViewer.swift's `pose(_:)`: hidden parts never draw
//! (`isEnabled`, :221-223), a part the state doesn't name holds identity
//! (:261), and both the translation and the quaternion's imaginary part
//! cross the same MODEL→SCENE `(x, z, −y)` remap (:262-263) — a rotation
//! about a MODEL axis is a rotation about the remapped scene axis by the
//! same angle. The composed matrix pivots on the scene pin, because
//! staged vertices carry that pin baked in while upstream rotates about
//! the un-pinned origin — see [`poses`] for the algebra.

use std::collections::BTreeSet;

use engine::parsing::animation::StudyState;

use crate::render::parts::remap;
use crate::render::{PartAppearance, PartPose, PartScene};

/// The speed menu's values, 0.1×..2× (MotionControls.swift:7).
pub const SPEEDS: [f64; 5] = [0.1, 0.25, 0.5, 1.0, 2.0];

/// The scrub slider's lower bound: the slider's range is
/// `0...max(duration, 0.001)` (MotionControls.swift:23), so even a
/// zero-duration study keeps a draggable — inert — slider.
pub const SCRUB_FLOOR: f64 = 0.001;

/// Playback state, MotionPlayer.swift:6-9 verbatim: a position along the
/// study, whether the clock runs, its speed multiplier, and whether
/// passing the end wraps. UI state only — the study itself is not held
/// here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionPlayer {
    pub time: f64,
    pub is_playing: bool,
    pub speed: f64,
    pub loops: bool,
}

impl Default for MotionPlayer {
    /// MotionPlayer.swift:6-9 field initializers: idle at 0, 1×, looping.
    fn default() -> Self {
        MotionPlayer {
            time: 0.0,
            is_playing: false,
            speed: 1.0,
            loops: true,
        }
    }
}

impl MotionPlayer {
    /// MotionPlayer.swift:11-22 verbatim: a paused player and a
    /// zero-duration study never move; otherwise `time += seconds·speed` —
    /// strictly below the duration keeps the time, at-or-past it wraps by
    /// remainder while looping (landing exactly on the duration wraps to
    /// 0: the compare is `<`), and clamps to the duration, stopping
    /// playback, when looping is off.
    pub fn advance(&mut self, seconds: f64, duration: f64) {
        if !self.is_playing || duration <= 0.0 {
            return;
        }
        let next = self.time + seconds * self.speed;
        if next < duration {
            self.time = next;
        } else if self.loops {
            // truncatingRemainder and Rust % agree: both keep the
            // dividend's sign
            self.time = next % duration;
        } else {
            self.time = duration;
            self.is_playing = false;
        }
    }

    /// MotionPlayer.swift:24-29: flipping to play restarts from 0 only
    /// when time sits at (or past) the duration; a mid-scrub resume keeps
    /// its time. Pausing never moves the time.
    pub fn toggle_playback(&mut self, duration: f64) {
        if !self.is_playing && self.time >= duration {
            self.time = 0.0;
        }
        self.is_playing = !self.is_playing;
    }
}

/// MODEL-space quaternion -> scene space: the SAME `(x, z, −y)` remap the
/// staging loop puts on translations, applied to the imaginary part
/// (upstream `simd_quatf(vector: SIMD4(sceneVector(rotate.imag),
/// rotate.real))`, ModelViewer.swift:263). Conjugating a rotation by the
/// remap's proper rotation maps its axis through the matrix and keeps the
/// angle, so a rotation about a MODEL axis becomes the same-angle rotation
/// about the remapped scene axis. Not renormalized, like upstream.
fn scene_quat(q: glam::Quat) -> glam::Quat {
    let imag = remap([q.x, q.y, q.z]);
    glam::Quat::from_xyzw(imag[0], imag[1], imag[2], q.w)
}

/// One study state -> one [`PartPose`] and one [`PartAppearance`] per
/// scene part, in `scene.parts` order (the renderer's per-part uniform
/// slots).
///
/// Pose (ModelViewer.swift:261-263): a part the state doesn't name holds
/// identity; `translate` crosses the MODEL→SCENE remap; `rotate` crosses
/// [`scene_quat`]. The matrix composes THROUGH the pin: staged vertices
/// carry `scene.pin` baked in (`PartScene::from_parts`'s staging loop),
/// while upstream's part entities hang under `content`, whose position is
/// that pin (ModelViewer.swift:168), and `pose()` writes position and
/// orientation in content's frame — so upstream world = `pin + q·v + t`.
/// A bare `from_rotation_translation(q, t)` over pinned vertices yields
/// `q·v + q·pin + t`, orbiting the part by `q·pin − pin` whenever the pin
/// is nonzero and the rotation isn't identity; adding `pin − q·pin` to
/// the translation lands every staged vertex exactly where upstream puts
/// it (and reduces to the bare form at identity rotation — which is why
/// every translation-only golden expectation stays valid).
///
/// Appearance (ModelViewer.swift:263-266, :282-284): a part named in any
/// of the state's collisions renders the flat-red material — opaque,
/// winning over translucency exactly as upstream's whole-array material
/// swap does; otherwise a part that never moves in the study rides the
/// 0.22 translucent material. `hidden` wins over everything: the pose
/// slot is `None` and the part never draws (`isEnabled`, :221-223).
pub fn poses(
    scene: &PartScene,
    state: &StudyState,
    moving: &BTreeSet<String>,
    hidden: &BTreeSet<String>,
) -> (Vec<PartPose>, Vec<PartAppearance>) {
    let colliding: BTreeSet<&str> = state
        .collisions
        .iter()
        .flat_map(|c| c.parts.iter().map(String::as_str))
        .collect();
    let pin = glam::Vec3::from(scene.pin);
    let mut poses = Vec::with_capacity(scene.parts.len());
    let mut looks = Vec::with_capacity(scene.parts.len());
    for part in &scene.parts {
        let collision = colliding.contains(part.name.as_str());
        looks.push(PartAppearance {
            collision,
            translucent: !collision && !moving.contains(&part.name),
        });
        if hidden.contains(&part.name) {
            poses.push(None);
            continue;
        }
        let pose = state.parts.get(&part.name).copied().unwrap_or_default();
        let quat = scene_quat(pose.rotate);
        let translate = glam::Vec3::from(remap(pose.translate)) + pin - quat * pin;
        poses.push(Some(
            glam::Mat4::from_rotation_translation(quat, translate).to_cols_array_2d(),
        ));
    }
    (poses, looks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::parsing::animation::MotionStudy;
    use std::collections::BTreeSet;

    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/golden")
            .join(rel)
    }

    fn golden_parts() -> Vec<engine::parsing::Part> {
        let bytes = std::fs::read(fixture("model.3mf")).unwrap();
        engine::parsing::threemf::parse_3mf(&bytes).unwrap()
    }

    fn golden_study() -> MotionStudy {
        MotionStudy::parse(&std::fs::read(fixture("animation.json")).unwrap()).unwrap()
    }

    #[test]
    fn advance_below_loops_and_clamps_like_upstream() {
        let mut p = MotionPlayer {
            is_playing: true,
            ..Default::default()
        };
        p.advance(0.5, 2.0);
        assert!((p.time - 0.5).abs() < 1e-9);
        // loop wrap uses remainder: 1.9 + 0.2 -> 0.1
        p.time = 1.9;
        p.advance(0.2, 2.0);
        assert!((p.time - 0.1).abs() < 1e-9, "wrapped: {}", p.time);
        // STRICT <: landing exactly on duration wraps to 0 while looping
        p.time = 1.5;
        p.advance(0.5, 2.0);
        assert!(p.time.abs() < 1e-9, "exact landing wraps: {}", p.time);
        // clamp when looping is off, and playback stops
        p.loops = false;
        p.time = 1.9;
        p.advance(0.2, 2.0);
        assert!((p.time - 2.0).abs() < 1e-9);
        assert!(!p.is_playing, "non-loop end stops playback");
        // paused advance is a no-op; so is duration 0
        p.is_playing = false;
        p.time = 0.25;
        p.advance(0.5, 2.0);
        assert!((p.time - 0.25).abs() < 1e-9);
        p.is_playing = true;
        p.advance(1.0, 0.0);
        assert!(
            (p.time - 0.25).abs() < 1e-9,
            "zero-duration study never advances"
        );
    }

    #[test]
    fn toggle_restarts_only_at_the_end() {
        let mut p = MotionPlayer {
            is_playing: false,
            time: 2.0,
            ..Default::default()
        };
        p.toggle_playback(2.0);
        assert!(p.is_playing && p.time == 0.0, "at duration: restart from 0");
        p.toggle_playback(2.0);
        assert!(!p.is_playing);
        p.time = 0.7;
        p.toggle_playback(2.0);
        assert!(
            p.is_playing && (p.time - 0.7).abs() < 1e-9,
            "mid-scrub resume keeps time"
        );
    }

    /// The load-bearing remap AND pivot: a MODEL rotation about MODEL +Z
    /// by 90° moves MODEL (1,0,0) to (0,1,0); staged scene space maps
    /// those to (1,0,0) and (0,0,-1) — so the scene quaternion must
    /// rotate scene x toward scene -z (an unremapped quaternion rotates
    /// toward scene +y, the wrong floor). And because staged vertices
    /// carry the pin baked in, the FULL matrix must land a staged point at
    /// `pin + q·remap(v)` — an unpinned `from_rotation_translation(q, t)`
    /// orbits it by `q·pin − pin` (here: MODEL (1,0,0) lands at
    /// (0.5, 0, -0.5) instead of (-0.5, 0, -0.5), a full unit off).
    #[test]
    fn model_rotation_maps_through_the_scene_remap() {
        let part = engine::parsing::Part {
            name: "lid".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&part), crate::render::NEUTRAL);
        let mut state = StudyState::default();
        state.parts.insert(
            "lid".to_string(),
            engine::parsing::animation::Pose {
                translate: [0.0; 3],
                rotate: glam::Quat::from_axis_angle(glam::Vec3::Z, std::f32::consts::FRAC_PI_2),
            },
        );
        let (poses, _) = poses(&scene, &state, &BTreeSet::new(), &BTreeSet::new());
        let m = glam::Mat4::from_cols_array_2d(&poses[0].expect("visible"));
        // direction: the scene quaternion rotates scene x toward scene -z
        let rotated = m.transform_vector3(glam::Vec3::new(1.0, 0.0, 0.0));
        assert!(
            (rotated - glam::Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "MODEL +Z quarter-turn maps to scene -z: {rotated:?}"
        );
        // FULL matrix over the PINNED scene: every staged vertex (remap(v) +
        // pin) must land at pin + q·remap(v). transform_point3 keeps the
        // translation term, which is exactly where the pivot error lives —
        // transform_vector3 strips it. q_scene is derived independently: a
        // rotation about MODEL +Z by θ is a rotation about the remapped
        // axis, scene +y, by the same θ.
        let pin = glam::Vec3::from(scene.pin);
        let remap = |v: [f32; 3]| glam::Vec3::new(v[0], v[2], -v[1]);
        let q_scene = glam::Quat::from_axis_angle(glam::Vec3::Y, std::f32::consts::FRAC_PI_2);
        for v in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let v_scene = remap(v);
            let landed = m.transform_point3(v_scene + pin);
            let expected = pin + q_scene * v_scene;
            assert!(
                (landed - expected).length() < 1e-5,
                "MODEL {v:?}: landed {landed:?} vs pin + q·remap(v) {expected:?}"
            );
        }
    }

    #[test]
    fn golden_poses_translucent_and_collision_states() {
        let scene = PartScene::from_parts(&golden_parts(), crate::render::NEUTRAL);
        let study = golden_study();
        let moving = study.moving_parts();
        // t = 0.5: lid lifted MODEL z 5 -> scene +y 5; base unnamed in the
        // study holds identity. The interpolation tie at the midpoint goes
        // to the LATER frame (upstream `fraction < 0.5 ? previous : next`),
        // so the t=1 collision is already live here.
        let (p, look) = poses(&scene, &study.state_at(0.5), &moving, &BTreeSet::new());
        assert_eq!(scene.parts[0].name, "base");
        assert_eq!(
            p[0],
            Some(glam::Mat4::IDENTITY.to_cols_array_2d()),
            "base holds identity"
        );
        let lid = glam::Mat4::from_cols_array_2d(&p[1].expect("lid visible"));
        let translation = lid.w_axis.truncate();
        assert!(
            (translation - glam::Vec3::new(0.0, 5.0, 0.0)).length() < 1e-4,
            "MODEL z 5 crosses to scene +y 5: {translation:?}"
        );
        assert!(
            look[0].collision && look[1].collision,
            "mid-span tie takes the later frame's collision"
        );
        // t = 0.4 (strictly inside the first span, before the tie): the
        // nearest frame is t=0, collision-free — the static base rides the
        // 0.22 translucent material, the moving lid stays opaque
        let (_, look) = poses(&scene, &study.state_at(0.4), &moving, &BTreeSet::new());
        assert!(
            look[0].translucent && !look[0].collision,
            "static base translucent"
        );
        assert!(
            !look[1].translucent && !look[1].collision,
            "moving lid opaque"
        );
        // t = 1.0: the golden frame carries base<->lid at volume 0.5
        let (_, look) = poses(&scene, &study.state_at(1.0), &moving, &BTreeSet::new());
        assert!(
            look[0].collision && look[1].collision,
            "both members flagged"
        );
        // a colliding STATIC part stays opaque: collision replaces translucency
        assert!(!look[0].translucent);
    }

    #[test]
    fn hidden_wins_over_pose_and_collision() {
        let scene = PartScene::from_parts(&golden_parts(), crate::render::NEUTRAL);
        let study = golden_study();
        let hidden = BTreeSet::from(["lid".to_string()]);
        let (p, _) = poses(&scene, &study.state_at(1.0), &study.moving_parts(), &hidden);
        assert!(
            p[1].is_none(),
            "hidden member never draws (ModelViewer.swift:221-223)"
        );
        assert!(p[0].is_some(), "the visible member still poses");
    }
}
