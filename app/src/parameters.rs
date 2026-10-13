//! Parameter overlay model (upstream Viewer3D/ParametersOverlay.swift): the
//! per-parameter slider state — live drag value, the range heuristic that
//! sizes the track, the commit rounding rules — plus the panel that tracks
//! the parameter set as rebuilds add and remove rows.
//!
//! Two rules are pinned to the Swift source and easy to confuse:
//! - the INTEGER commit rule keys on the parameter's COMMITTED value
//!   (`isInteger` reads `parameter.value`, ParametersOverlay.swift:134-136),
//!   but it ROUNDS the slider's live value — a parameter at 3 with the thumb
//!   at 4.4 commits 4, not 4.4 or 3;
//! - [`SliderState::sync_from`] re-fits the range ONLY when the incoming
//!   value escapes it (ParametersOverlay.swift:172-178); a contained value
//!   keeps the range the user has been dragging in.
//!
//! Upstream's `commit` also re-fits the range in place when the committed
//! value escapes it (ParametersOverlay.swift:184-187). Here the state stays
//! pure (`commit` is `&self`): the escape can only come from the TEXT FIELD
//! (the slider widget clamps its own drags to the range), and the mount
//! follows a commit with [`SliderState::sync_from`], which performs the same
//! re-fit — idempotent when the parameter round-trip later syncs the same
//! value.
//!
//! This module is GUI-free (the crate's purity rule): no cosmic types.

use engine::project::parameters::ModelParameter;

/// One parameter's slider: the live (possibly mid-drag) value, the track
/// range, and whether a drag is in progress (the external-sync gate).
#[derive(Debug, Clone, PartialEq)]
pub struct SliderState {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub editing: bool,
}

impl SliderState {
    /// State around `value`'s heuristic range.
    pub fn new(value: f64) -> Self {
        let (min, max) = Self::range_around(value);
        Self {
            value,
            min,
            max,
            editing: false,
        }
    }

    /// The track range for a value (ParametersOverlay.swift:190-198):
    /// positive values span `0…max(2v, 1)`, negative `2v…max(-2v, 1)`
    /// (note the asymmetry — a negative value's upper bound is also
    /// `max(-2v, 1)`), and zero gets the neutral `-10…10`.
    pub fn range_around(value: f64) -> (f64, f64) {
        if value > 0.0 {
            (0.0, (value * 2.0).max(1.0))
        } else if value < 0.0 {
            (value * 2.0, (-value * 2.0).max(1.0))
        } else {
            (-10.0, 10.0)
        }
    }

    /// Whether `committed` takes the integer commit rule
    /// (ParametersOverlay.swift:134-136): integral and `|v| >= 1`.
    pub fn is_integer_value(committed: f64) -> bool {
        committed == committed.round() && committed.abs() >= 1.0
    }

    /// The value a commit of the live `self.value` produces when the
    /// parameter sits at `current`: the rule is chosen by the COMMITTED
    /// value and applied to the LIVE one (see the module docs).
    pub fn commit_value(&self, current: f64) -> f64 {
        if Self::is_integer_value(current) {
            self.value.round()
        } else {
            (self.value * 1000.0).round() / 1000.0
        }
    }

    /// The commit, or `None` when it would not change the parameter
    /// (upstream's `guard rounded != parameter.value`).
    pub fn commit(&self, current: f64) -> Option<f64> {
        let value = self.commit_value(current);
        (value != current).then_some(value)
    }

    /// Follow an external parameter change: mid-drag edits are kept, the
    /// value follows otherwise, and the range re-fits only on escape
    /// (ParametersOverlay.swift:172-178).
    pub fn sync_from(&mut self, value: f64) {
        if self.editing {
            return;
        }
        self.value = value;
        if !(self.min..=self.max).contains(&value) {
            (self.min, self.max) = Self::range_around(value);
        }
    }
}

/// The overlay's rows: the collapse state plus one slider per parameter, in
/// the order the parameters arrive (upstream renders `project.parameters`,
/// which the engine's `ordered_parameters` keeps in script order).
#[derive(Debug, Clone, PartialEq)]
pub struct ParametersPanel {
    pub collapsed: bool,
    pub sliders: Vec<(String, SliderState)>,
}

impl ParametersPanel {
    /// A panel over `params` (each row starts at its parameter's committed
    /// value), carrying `collapsed` from the persisted setting.
    pub fn from_parameters(params: &[ModelParameter], collapsed: bool) -> Self {
        Self {
            collapsed,
            sliders: params
                .iter()
                .map(|p| (p.name.clone(), SliderState::new(p.value)))
                .collect(),
        }
    }

    /// Follow the parameter set: surviving rows keep their slider state
    /// (range included — only an escaping value re-fits it, via
    /// [`SliderState::sync_from`]), new parameters append, removed ones
    /// drop, and the order follows `params`.
    pub fn sync(&mut self, params: &[ModelParameter]) {
        let mut next = Vec::with_capacity(params.len());
        for param in params {
            let mut slider = self
                .sliders
                .iter()
                .find(|(name, _)| name == &param.name)
                .map(|(_, slider)| slider.clone())
                .unwrap_or_else(|| SliderState::new(param.value));
            slider.sync_from(param.value);
            next.push((param.name.clone(), slider));
        }
        self.sliders = next;
    }
}

/// Swift's `.number.precision(.fractionLength(0...max))`: at most `max`
/// fraction digits, trailing zeros (and a bare trailing point) trimmed —
/// the previous value formats with 2, the live field with 3. The trim
/// never touches the integer part (`100` stays `100`).
pub fn format_display(value: f64, max_fraction_digits: usize) -> String {
    let formatted = format!("{value:.max_fraction_digits$}");
    let Some(dot) = formatted.find('.') else {
        return formatted;
    };
    let kept = formatted[dot + 1..].trim_end_matches('0').len();
    let mut out = formatted[..dot + 1 + kept].to_string();
    if out.ends_with('.') {
        out.pop();
    }
    out
}
