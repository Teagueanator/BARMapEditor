//! Build pipeline — turn a [`barme_core::Project`] + on-disk asset PNG/BMP into
//! the artefacts Recoil consumes.
//!
//! - [`pymapconv`] — subprocess driver around the vendored PyMapConv binary
//!   (ADR-012). Produces `.smf` + `.smt`.
//!
//! `mapinfo.lua` emit and the non-solid `.sd7` packager land in the next
//! commit (ADR-013); their modules will be added here when they do.

pub mod pymapconv;

pub use pymapconv::{CompileInputs, CompileOutputs, PyMapConvDriver, PyMapConvError};
