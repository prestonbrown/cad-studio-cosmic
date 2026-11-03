//! The shared view kit every window and pane draws with.
//!
//! - [`tone`]: text and glyph colors by role (primary, secondary, …,
//!   accent, the status colors), resolved from the theme with one alpha
//!   table per backdrop.
//! - [`icon`]: the SF Symbols registry and the one icon constructor.
//! - [`surface`]: the material, cards, capsules, washes, dividers, and
//!   corner radii by COSMIC roundness class.
//! - [`segmented`]: the small segmented control.
//! - [`button`]: borderless icon/text buttons and the toolbar look.
//!
//! The kit decides colors, radii and enablement; each caller lays out its
//! own view with them. Only what at least two views share lives here.

pub mod button;
pub mod icon;
pub mod segmented;
pub mod surface;
pub mod tone;

pub use icon::Symbol;
pub use surface::Corner;
pub use tone::{Backdrop, Tone};
