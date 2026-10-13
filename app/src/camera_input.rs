//! Camera input state machine — the port of upstream `CameraInputView.swift`
//! over the viewer's parity camera formulas (`viewer::render::camera`,
//! facts §1). Owns the pose (yaw/pitch/distance/target) plus the gesture
//! bookkeeping: press/release with the click-vs-drag accumulator, drag decode
//! (orbit vs pan), scroll decode (precise `/12` steps, `y`-then-`x` fallback),
//! pinch magnify, presets with re-frame, fit, and the re-frame-on->50 %-diagonal
//! mesh-change rule (ModelViewerState.swift:127-135).
//!
//! Pure: no cosmic/iced types, no I/O — inputs arrive as plain numbers in
//! iced's conventions and the AppKit-convention flips happen HERE, at the
//! boundary, so the pane never re-derives them:
//! - cursor-derived `dy` (drag) is TOP-DOWN in iced and BOTTOM-UP in AppKit —
//!   negated once on the way into the formulas (facts #18/#29);
//! - scroll deltas pass through UNCHANGED — iced's wheel conversion forwards
//!   winit's deltas, which carry AppKit's `scrollingDelta` sign, the same
//!   assumption the M3-settled zoom decode already makes.
//!
//! Pinch/magnify (facts #116, this module's to-own carry-forward): the
//! formula is `zoom(1 / max(1 + magnification, 0.1))` through the same
//! radius-relative clamp as scroll. The iced fork at the workspace's pin
//! translates no magnify event (winit's `PinchGesture` is dropped in
//! `iced_winit`'s conversion), so no platform event reaches
//! [`CameraInput::magnify`] today — the state machine and the pane message
//! exist so the wiring is one match arm when the runtime grows the event.

use viewer::render::camera::{self, Preset};

/// Click-vs-drag threshold in px (CameraInputView.swift:37,40-52): a press
/// whose accumulated `|dx| + |dy|` stays strictly under this counts as a
/// click on release.
pub const CLICK_THRESHOLD: f32 = 4.0;
/// Zoom per scroll step (CameraInputView.swift:69): `pow(0.9, steps)`.
pub const ZOOM_STEP_BASE: f32 = 0.9;
/// Pixels per zoom step for precise (trackpad) deltas
/// (CameraInputView.swift:68).
pub const PRECISE_STEP_PIXELS: f32 = 12.0;
/// The pinch denominator floor (CameraInputView.swift:75):
/// `1 / max(1 + magnification, 0.1)`.
pub const MAGNIFY_FLOOR: f32 = 0.1;
/// The re-frame threshold: a new mesh reframes only when its diagonal
/// moved more than this fraction of the framed one
/// (ModelViewerState.swift:127-135).
pub const REFRAME_DIAGONAL_FRACTION: f32 = 0.5;

/// One scroll event's zoom steps (facts #20): `y` is the wheel and `x` the
/// fallback when `y` is zero; precise (trackpad pixel) deltas count
/// [`PRECISE_STEP_PIXELS`] px per step, notched line deltas are steps
/// already.
pub fn scroll_steps(x: f32, y: f32, precise: bool) -> f32 {
    let d = if y != 0.0 { y } else { x };
    if precise { d / PRECISE_STEP_PIXELS } else { d }
}

/// The pinch zoom factor (CameraInputView.swift:73-75):
/// `1 / max(1 + magnification, 0.1)` — a spread (positive magnification)
/// shrinks the distance (zoom in), a pinch does the reverse, and the floor
/// keeps a near −1 magnification from dividing by ~0.
pub fn magnify_factor(magnification: f32) -> f32 {
    1.0 / (1.0 + magnification).max(MAGNIFY_FLOOR)
}

/// The orbit/pan camera pose plus gesture state. The pane constructs it
/// against the scene aabb (iso preset, parity framing), feeds it iced's
/// raw numbers, and pushes the settled pose into the viewer's `ModelScene`
/// after every event.
pub struct CameraInput {
    /// Orbit yaw, radians. Decreases with rightward drag.
    pub yaw: f32,
    /// Orbit pitch, radians, clamped to ±[`camera::MAX_PITCH`].
    pub pitch: f32,
    /// Camera distance from `target` — scroll and pinch zoom this itself,
    /// clamped to [0.1·radius, 200·radius].
    pub distance: f32,
    /// Look-at target (scene space).
    pub target: [f32; 3],
    /// The scene aabb presets and fit re-frame against.
    aabb: ([f32; 3], [f32; 3]),
    /// Framing radius of the current scene — the zoom clamp scales it.
    radius: f32,
    /// Size vector of the aabb the camera last framed against; the > 50 %
    /// diagonal rule compares new meshes to it. Set at construction (the
    /// initial frame) and by every re-frame.
    framed_size: [f32; 3],
    /// Accumulated `|dx| + |dy|` since the press (click-vs-drag).
    drag_distance: f32,
    /// The open drag pans instead of orbiting (right/middle button or a
    /// shift/option modifier).
    drag_pan: bool,
    pressed: bool,
    /// Last cursor position in iced's top-down coordinates.
    last: Option<(f32, f32)>,
    /// Viewport height in px — the pan formula's world-per-pixel scale.
    view_height: f32,
}

impl CameraInput {
    /// The iso preset framing `aabb` (upstream's initial camera:
    /// ModelViewer.swift:19-33, 328-337).
    pub fn new(aabb_min: [f32; 3], aabb_max: [f32; 3]) -> Self {
        let mut input = Self {
            yaw: 0.0,
            pitch: 0.0,
            distance: 0.0,
            target: [0.0; 3],
            aabb: (aabb_min, aabb_max),
            radius: 0.0,
            framed_size: [0.0; 3],
            drag_distance: 0.0,
            drag_pan: false,
            pressed: false,
            last: None,
            view_height: 0.0,
        };
        input.reframe_to(aabb_min, aabb_max, true);
        (input.yaw, input.pitch) = Preset::Iso.yaw_pitch();
        input
    }

    /// A press started — `pan` is true for right/middle buttons or a
    /// shift/option modifier on the primary one (CameraInputView.swift:
    /// 40-52,55-61). Resets the click-vs-drag accumulator.
    pub fn press(&mut self, pan: bool) {
        self.pressed = true;
        self.drag_pan = pan;
        self.drag_distance = 0.0;
        self.last = None;
    }

    /// The cursor moved to iced-convention `(x, y)` (top-down). Hover moves
    /// (no press) are ignored; the first move after a press only anchors
    /// the reference point.
    pub fn move_to(&mut self, x: f32, y: f32) {
        if !self.pressed {
            return;
        }
        if let Some((lx, ly)) = self.last {
            let (dx, dy) = (x - lx, y - ly);
            self.drag_distance += dx.abs() + dy.abs();
            // AppKit deltas are bottom-up: the parity formulas' dy is the
            // negation of iced's top-down one (facts #18/#29)
            if self.drag_pan {
                self.pan(dx, -dy);
            } else {
                (self.yaw, self.pitch) = camera::orbit_drag(self.yaw, self.pitch, dx, -dy);
            }
        }
        self.last = Some((x, y));
    }

    /// The button went up: true when the press was a CLICK — accumulated
    /// travel strictly under [`CLICK_THRESHOLD`] (CameraInputView.swift:40-52).
    /// A release with no press open is not one.
    pub fn release(&mut self) -> bool {
        let was_click = self.pressed && self.drag_distance < CLICK_THRESHOLD;
        self.pressed = false;
        self.last = None;
        was_click
    }

    /// A scroll wheel/trackpad event: decoded `(x, y)` delta and whether it
    /// was precise (pixel) mode. `pan` routes it through the pan formula
    /// instead of zoom (CameraInputView.swift:63-71).
    pub fn scroll(&mut self, x: f32, y: f32, precise: bool, pan: bool) {
        if pan {
            // scroll deltas carry AppKit's scrollingDelta sign already (the
            // M3-settled zoom decode assumes the same), so they pass
            // through unflipped
            self.pan(x, y);
        } else {
            self.zoom(ZOOM_STEP_BASE.powf(scroll_steps(x, y, precise)));
        }
    }

    /// A pinch magnify delta (CameraInputView.swift:73-75) — zooms through
    /// the same radius-relative clamp as scroll.
    pub fn magnify(&mut self, magnification: f32) {
        self.zoom(magnify_factor(magnification));
    }

    /// Switch to `preset` and re-frame (upstream's preset change bumps
    /// `cameraID`, ModelViewer.swift:322-326).
    pub fn preset(&mut self, preset: Preset) {
        let (aabb_min, aabb_max) = self.aabb;
        self.reframe_to(aabb_min, aabb_max, true);
        (self.yaw, self.pitch) = preset.yaw_pitch();
    }

    /// Re-frame the current scene, keeping the angles (upstream's fit).
    pub fn fit(&mut self) {
        let (aabb_min, aabb_max) = self.aabb;
        self.reframe_to(aabb_min, aabb_max, true);
    }

    /// The scene became `aabb` (an output reload): stores the framing
    /// reference, always refreshes the zoom-clamp radius, and re-frames
    /// (target + distance from [`camera::frame`], angles kept) only when
    /// the diagonal moved more than [`REFRAME_DIAGONAL_FRACTION`] of the
    /// last-framed one — returns whether that happened
    /// (ModelViewerState.swift:127-135). The construction frame counts as
    /// the first framed size, so a reload is always measured against the
    /// scene on screen.
    pub fn mesh_changed(&mut self, aabb_min: [f32; 3], aabb_max: [f32; 3]) -> bool {
        let size = size_of(aabb_min, aabb_max);
        let previous = diagonal(self.framed_size).max(0.001);
        let reframe = (diagonal(size) - previous).abs() / previous > REFRAME_DIAGONAL_FRACTION;
        self.reframe_to(aabb_min, aabb_max, reframe);
        reframe
    }

    /// The viewport's pixel height — the pan formula's scale
    /// (ModelViewer.swift:312-320). Reported from layout.
    pub fn set_view_height(&mut self, height: f32) {
        self.view_height = height;
    }

    /// Whether a drag is open (the cursor interaction to display).
    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// distance × `factor`, clamped to [0.1·radius, 200·radius]
    /// (ModelViewer.swift:307-310).
    fn zoom(&mut self, factor: f32) {
        self.distance = camera::zoomed(self.distance, factor, self.radius);
    }

    /// target += (-right·dx + up·dy)·units_per_point over the camera's own
    /// scene axes, `dx`/`dy` in AppKit convention (bottom-up dy).
    fn pan(&mut self, dx: f32, dy: f32) {
        self.target = camera::panned(
            self.target,
            self.yaw,
            self.pitch,
            self.distance,
            self.view_height,
            dx,
            dy,
        );
    }

    /// Adopt `aabb` as the framing reference: recompute radius, store the
    /// size vector, and (when `frame_too`) reset target/distance from
    /// [`camera::frame`].
    fn reframe_to(&mut self, aabb_min: [f32; 3], aabb_max: [f32; 3], frame_too: bool) {
        self.aabb = (aabb_min, aabb_max);
        self.radius = camera::framing_radius(aabb_min, aabb_max);
        self.framed_size = size_of(aabb_min, aabb_max);
        if frame_too {
            let (target, distance) = camera::frame(aabb_min, aabb_max);
            self.target = target;
            self.distance = distance;
        }
    }
}

/// An aabb's extent vector.
fn size_of(aabb_min: [f32; 3], aabb_max: [f32; 3]) -> [f32; 3] {
    [
        aabb_max[0] - aabb_min[0],
        aabb_max[1] - aabb_min[1],
        aabb_max[2] - aabb_min[2],
    ]
}

/// A size vector's diagonal length — the re-frame rule's measure (the
/// remap permutes it, so MODEL and SCENE agree here).
fn diagonal(size: [f32; 3]) -> f32 {
    (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;
    use viewer::render::camera::{FOV, MAX_PITCH};

    /// The golden-rule aabb: a 30×40×0 plate has diagonal 50, radius 25.
    fn plate_aabb() -> ([f32; 3], [f32; 3]) {
        ([0.0; 3], [30.0, 40.0, 0.0])
    }

    fn input() -> CameraInput {
        let (min, max) = plate_aabb();
        CameraInput::new(min, max)
    }

    fn approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "approx: {actual} vs {expected}"
        );
    }

    fn approx3(actual: [f32; 3], expected: [f32; 3]) {
        for (a, e) in actual.iter().zip(expected) {
            approx(*a, e);
        }
    }

    #[test]
    fn frame_formula_is_half_diagonal_over_half_fov() {
        // frame() (ModelViewer.swift:328-337): target at half the scene
        // height above the pinned floor, distance = radius/sin(fov/2)*1.15
        // with radius = diagonal/2 = 25 — not max-extent/2 (facts #23).
        let cam = input();
        let (_, distance) = camera::frame([0.0; 3], [30.0, 40.0, 0.0]);
        approx(cam.distance, distance);
        approx(distance, 25.0 / (FOV / 2.0).sin() * 1.15);
        approx3(cam.target, [0.0, 20.0, 0.0]); // half the scene height
    }

    #[test]
    fn orbit_drag_matches_sensitivity_formula_with_y_flip() {
        // facts #18: yaw -= dx*0.008, pitch += dy*0.008 (AppKit dy,
        // bottom-up). Iced's cursor y is top-down, so dragging UP (iced
        // dy = -50) must RAISE the pitch by 0.4.
        let mut cam = input();
        cam.press(false);
        cam.move_to(100.0, 100.0);
        cam.move_to(180.0, 50.0); // dx = 80, iced dy = -50 -> AppKit dy = +50
        let (yaw, pitch) = (cam.yaw, cam.pitch);
        approx(yaw, std::f32::consts::FRAC_PI_4 - 80.0 * 0.008);
        approx(pitch, 0.6 + 50.0 * 0.008);
    }

    #[test]
    fn pitch_clamps_at_upstream_max() {
        let mut cam = input();
        cam.press(false);
        cam.move_to(0.0, 0.0);
        cam.move_to(0.0, -2000.0); // AppKit dy = +2000 -> 16 rad, clamped
        approx(cam.pitch, MAX_PITCH);
        cam.release();
        cam.press(false);
        cam.move_to(0.0, 0.0);
        cam.move_to(0.0, 2000.0); // AppKit dy = -2000
        approx(cam.pitch, -MAX_PITCH);
    }

    #[test]
    fn hover_moves_are_ignored() {
        let mut cam = input();
        let (yaw, pitch, distance, target) = (cam.yaw, cam.pitch, cam.distance, cam.target);
        cam.move_to(10.0, 10.0);
        cam.move_to(400.0, -300.0);
        approx(cam.yaw, yaw);
        approx(cam.pitch, pitch);
        approx(cam.distance, distance);
        approx3(cam.target, target);
    }

    #[test]
    fn scroll_steps_decode_precise_and_lines() {
        // facts #20: precise deltas count 12 px per step; line deltas are
        // steps already; x is the fallback when y is zero.
        approx(scroll_steps(0.0, 24.0, true), 2.0);
        approx(scroll_steps(0.0, 3.0, false), 3.0);
        approx(scroll_steps(18.0, 0.0, true), 1.5);
        approx(scroll_steps(-5.0, 0.0, false), -5.0);
    }

    #[test]
    fn scroll_zoom_multiplies_distance_by_step_factor() {
        let mut cam = input();
        let d0 = cam.distance;
        cam.scroll(0.0, 24.0, true, false); // 2 precise steps
        approx(cam.distance, d0 * 0.9f32.powf(2.0));
        cam.scroll(0.0, 3.0, false, false); // 3 line steps the other way
        approx(cam.distance, d0 * 0.9f32.powf(2.0) * 0.9f32.powf(3.0));
    }

    #[test]
    fn zoom_clamps_are_radius_relative() {
        // facts #19: [0.1*radius, 200*radius] with radius = diagonal/2.
        let mut cam = input();
        let radius = 25.0;
        cam.scroll(0.0, f32::MAX / 12.0, true, false); // zoom in, huge
        approx(cam.distance, radius * 0.1);
        cam.scroll(0.0, -f32::MAX / 12.0, true, false); // zoom out, huge
        approx(cam.distance, radius * 200.0);
    }

    #[test]
    fn pan_uses_camera_axes_and_units_per_point() {
        // facts #22 at yaw = 0, pitch = 0: back = (0,0,1), right = (1,0,0),
        // up = (0,1,0); dragging right pans the target LEFT and dragging up
        // (iced dy negative, AppKit positive) pans it UP.
        let mut cam = input();
        cam.preset(Preset::Front);
        cam.set_view_height(600.0);
        let (t0, d0) = (cam.target, cam.distance);
        let upp = 2.0 * d0 * (FOV / 2.0).tan() / 600.0;
        cam.press(true); // right-drag pans (CameraInputView.swift:55-61)
        cam.move_to(0.0, 0.0);
        cam.move_to(30.0, -40.0); // dx = 30, AppKit dy = +40
        approx3(cam.target, [t0[0] - 30.0 * upp, t0[1] + 40.0 * upp, t0[2]]);
        approx(cam.distance, d0); // pan never zooms
    }

    #[test]
    fn modifier_scroll_pans_instead_of_zooming() {
        // CameraInputView.swift:63-66: shift/option + scroll pans by the
        // scroll delta itself (AppKit-sign dy, passed through unchanged).
        let mut cam = input();
        cam.preset(Preset::Front);
        cam.set_view_height(600.0);
        let (t0, d0) = (cam.target, cam.distance);
        let upp = 2.0 * d0 * (FOV / 2.0).tan() / 600.0;
        cam.scroll(20.0, -30.0, true, true);
        approx(cam.distance, d0);
        approx3(cam.target, [t0[0] - 20.0 * upp, t0[1] - 30.0 * upp, t0[2]]);
    }

    #[test]
    fn release_click_threshold_is_strict() {
        // facts #27: click iff accumulated |dx|+|dy| < 4 px, summed over the
        // whole drag path, and strictly less.
        let mut cam = input();
        cam.press(false);
        cam.move_to(0.0, 0.0);
        cam.move_to(2.0, 1.0); // travel 3
        cam.move_to(4.0, 1.5); // + 2.5 more -> 5.5 total
        assert!(!cam.release());

        let mut cam = input();
        cam.press(false);
        cam.move_to(0.0, 0.0);
        cam.move_to(3.5, 0.0); // exactly 3.5 < 4
        assert!(cam.release());

        // exactly 4.0 is NOT a click (strict <)
        cam.press(false);
        cam.move_to(0.0, 0.0);
        cam.move_to(4.0, 0.0);
        assert!(!cam.release());

        // release without a press is not a click
        let mut cam = input();
        assert!(!cam.release());
    }

    #[test]
    fn magnify_formula_and_floor() {
        // CameraInputView.swift:73-75.
        approx(magnify_factor(0.5), 1.0 / 1.5); // spread => zoom in
        approx(magnify_factor(-0.5), 1.0 / 0.5); // contract => zoom out
        approx(magnify_factor(-0.95), 1.0 / MAGNIFY_FLOOR); // floor at 0.1
    }

    #[test]
    fn magnify_zooms_under_the_same_clamp() {
        let mut cam = input();
        let d0 = cam.distance;
        cam.magnify(0.5); // factor 2/3 -> closer
        approx(cam.distance, (d0 * (1.0 / 1.5)).max(25.0 * 0.1));
        cam.scroll(0.0, -f32::MAX / 12.0, true, false); // park at 200r
        cam.magnify(-0.95); // factor 10, but clamped at 200*radius
        approx(cam.distance, 25.0 * 200.0);
    }

    #[test]
    fn presets_set_angles_and_reframe() {
        // facts #25: iso (pi/4, 0.6), front (0, 0), top (0, MAX_PITCH),
        // right (pi/2, 0); every preset change re-frames.
        let mut cam = input();
        for (preset, yaw, pitch) in [
            (Preset::Iso, std::f32::consts::FRAC_PI_4, 0.6),
            (Preset::Front, 0.0, 0.0),
            (Preset::Top, 0.0, MAX_PITCH),
            (Preset::Right, FRAC_PI_2, 0.0),
        ] {
            cam.preset(preset);
            approx(cam.yaw, yaw);
            approx(cam.pitch, pitch);
            let (target, distance) = camera::frame([0.0; 3], [30.0, 40.0, 0.0]);
            approx(cam.distance, distance);
            approx3(cam.target, target);
        }
    }

    #[test]
    fn fit_keeps_angles_and_reframes() {
        let mut cam = input();
        cam.preset(Preset::Right);
        cam.scroll(0.0, 24.0, true, false); // zoom off the preset distance
        cam.fit();
        approx(cam.yaw, FRAC_PI_2); // angles kept
        let (_, distance) = camera::frame([0.0; 3], [30.0, 40.0, 0.0]);
        approx(cam.distance, distance); // distance reset
    }

    #[test]
    fn mesh_change_over_half_diagonal_reframes() {
        // ModelViewerState.swift:127-135: |diag(new) - diag(prev)|/prev > 0.5
        let mut cam = input();
        // drag the camera off the framed pose so a re-frame is observable
        cam.preset(Preset::Front);
        cam.scroll(0.0, 24.0, true, false);
        let (target_before, distance_before) = (cam.target, cam.distance);

        // diagonal 50 -> 60: |60-50|/50 = 0.2, under threshold
        assert!(!cam.mesh_changed([0.0; 3], [30.0, 48.0, 0.0]));
        approx3(cam.target, target_before);
        approx(cam.distance, distance_before);

        // diagonal 60 -> 120: ratio 1.0, over threshold
        assert!(cam.mesh_changed([0.0; 3], [96.0, 48.0, 0.0]));
        let (_, distance) = camera::frame([0.0; 3], [96.0, 48.0, 0.0]);
        approx(cam.distance, distance);
        approx3(cam.target, [0.0, 24.0, 0.0]);
    }

    #[test]
    fn mesh_change_always_refreshes_the_clamp_radius() {
        // Even under the threshold the zoom clamp follows the NEW scene
        // (upstream zoom(by:) reads the current mesh's radius).
        let mut cam = input(); // diagonal 50, radius 25
        let (min, max) = ([0.0; 3], [48.0, 48.0, 0.0]); // diagonal ~67.9
        assert!(!cam.mesh_changed(min, max), "ratio 0.36 stays under 0.5");
        cam.scroll(0.0, -f32::MAX / 12.0, true, false); // zoom out hard
        let radius = camera::framing_radius(min, max);
        approx(cam.distance, radius * 200.0); // the NEW radius, not 25
    }

    #[test]
    fn mesh_change_measures_against_the_construction_frame() {
        // The construction frame is the first framed size: a reload very
        // different from what `new` framed re-frames, an identical one
        // never does.
        let mut cam = input();
        assert!(!cam.mesh_changed([0.0; 3], [30.0, 40.0, 0.0]));
        assert!(cam.mesh_changed([0.0; 3], [300.0, 400.0, 0.0]));
        let (_, distance) = camera::frame([0.0; 3], [300.0, 400.0, 0.0]);
        approx(cam.distance, distance);
    }
}
