//! The Python toolchain: venv lifecycle and install planning ([`env`]), the
//! model.py runner ([`model`]), and the warm builder ([`warm`]) — a
//! fork-per-build server that pre-imports build123d and matplotlib so a
//! model.py run starts at the geometry, not the imports.

pub mod env;
pub mod model;
pub mod warm;
