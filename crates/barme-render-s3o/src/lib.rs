//! Sprint 29b / R5 / Phase B — Recoil S3O model parser, offscreen
//! thumbnail render pass, and content-addressed PNG cache.
//!
//! This crate turns an upstream `mapfeatures/objects3d/<name>.s3o`
//! file into a 128² RGBA8 thumbnail the marker pipeline binds as a
//! [`MarkerShape::TexturedSprite`](../barme_app/ui/markers/enum.MarkerShape.html)
//! layer. The crate is consumed by `barme-app::FeatureCatalog::
//! populate_decal_registry` at app startup: every catalog entry with
//! a resolvable `.s3o` (per the per-family `s3o_pattern` or per-entry
//! `s3o` override) goes through this crate.
//!
//! Scope split per ADR-046 (Phase A) → ADR-047 (this sprint, Phase B):
//!
//! | concern                              | Phase A                | Phase B (this crate)            |
//! | ------------------------------------ | ---------------------- | ------------------------------- |
//! | what each layer shows                | family diffuse decal   | per-entry 3D thumbnail          |
//! | source data                          | `unittextures/*.tga`   | `objects3d/*.s3o` + same tga    |
//! | atlas size                           | 32 layers              | 128 layers (bumped this sprint) |
//! | per-entry distinct geometry visible  | no (all variants flat) | yes                             |
//!
//! ## Why a separate crate
//!
//! 1. Keeps the S3O binary parser + its fuzz / fixture tests away
//!    from the editor's per-frame paths.
//! 2. The thumbnail bake runs on a wgpu device but doesn't need any
//!    editor-side state (no Project, no UI), so a leaf crate is the
//!    right home.
//! 3. The cache module is pure file I/O + hashing — useful in
//!    isolation for the eventual headless thumbnail-bake CLI.
//!
//! ## Public surface
//!
//! - [`parser::parse_s3o`] — `&[u8] → S3oModel` (deterministic).
//! - [`thumbnail::bake_thumbnail`] — `&S3oModel + &diffuse_rgba →
//!   128² RGBA8` (lazy GPU init; offscreen ortho pass).
//! - [`cache::lookup`] / [`cache::store`] — content-addressed PNG
//!   round-trip rooted at `$XDG_CACHE_HOME/barme/feature_thumbnails/`.

pub mod cache;
pub mod parser;
pub mod thumbnail;
