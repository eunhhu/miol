//! Shared compact identifiers used across the orv pipeline.
//!
//! This crate intentionally has no dependencies so early and late pipeline
//! crates can share IDs without pulling parser or resolver layers into artifact
//! generation crates.

#![warn(missing_docs)]

/// Unique binding identifier assigned by name resolution and consumed by HIR,
/// compiler artifacts, and the reference runtime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NameId(pub u32);
