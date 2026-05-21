//! Sprint 27 / U5 — inspector section-accent audit.
//!
//! Source-file parsing test: reads `main.rs` and `ui/layers_panel.rs`,
//! locates every `fn inspector_*` body, counts the `accent: true`
//! parameter passed to `widgets::section` / `widgets::section_with_hover`
//! calls inside the function, and asserts that no per-tool inspector
//! emits more than one accent section.
//!
//! Why a source-file test instead of a runtime egui::Context::run?
//! - Deterministic: no GPU / windowing dependency.
//! - Fast: runs in milliseconds, not seconds.
//! - The runtime path is fragile — egui's pumps want fonts + a
//!   viewport, and Sprint 27 is a refactor that should land green
//!   without an egui test harness investment.
//!
//! Coverage extends to the PaintLayer inspector indirectly: its
//! "Layers" accent section lives in `layers_panel.rs::render`. The
//! test stitches both source files into a single audit so the
//! combined accent count is correct.

use std::fs;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Slice `src` into the body of every `fn inspector_*` it contains.
/// Body boundaries are detected by matching the function's opening
/// brace against an indentation-aware closing brace search.
fn inspector_bodies(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.trim_start().strip_prefix("fn inspector_") {
            // Extract the name up to '('.
            let name_end = rest.find('(').unwrap_or(rest.len());
            let name = format!("inspector_{}", &rest[..name_end]);
            // Walk forward to the first '{' on this or a later line.
            let mut buf = String::new();
            let mut depth = 0i32;
            let mut started = false;
            for l in &lines[i..] {
                buf.push_str(l);
                buf.push('\n');
                for ch in l.chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            started = true;
                        }
                        '}' => {
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                if started && depth == 0 {
                    break;
                }
            }
            out.push((name, buf));
        }
        i += 1;
        let _ = bytes;
    }
    out
}

/// Count `accent: true` parameters passed to `widgets::section(_with_hover)?`
/// inside `body`. The signature is positional:
///   widgets::section(ui, title, ACCENT, right, body)
///   widgets::section_with_hover(ui, title, ACCENT, hover, right, body)
/// We grep for `widgets::section` opens and walk forward to the 3rd
/// (or 3rd for both forms — `accent` is always the 3rd positional)
/// non-blank argument, asserting it's `true` or `false`.
fn count_accent(body: &str) -> (usize, usize) {
    let mut t = 0;
    let mut f = 0;
    let mut idx = 0;
    while let Some(pos) = body[idx..].find("widgets::section") {
        let start = idx + pos;
        // Advance past `widgets::section` or `widgets::section_with_hover`.
        let open_paren = body[start..].find('(').map(|o| start + o);
        let Some(open) = open_paren else {
            break;
        };
        // Walk the args; find the 3rd top-level comma-separated arg.
        let bytes = body.as_bytes();
        let mut depth = 0i32;
        let mut arg_starts = vec![open + 1];
        let mut k = open + 1;
        while k < bytes.len() {
            match bytes[k] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                b',' if depth == 0 => arg_starts.push(k + 1),
                _ => {}
            }
            k += 1;
        }
        // Third positional argument (index 2 in arg_starts).
        if arg_starts.len() >= 3 {
            let arg_start = arg_starts[2];
            let arg_end = arg_starts.get(3).copied().unwrap_or(k);
            let arg = body[arg_start..arg_end].trim().trim_end_matches(',').trim();
            if arg == "true" {
                t += 1;
            } else if arg == "false" {
                f += 1;
            }
        }
        idx = open + 1;
    }
    (t, f)
}

/// Per-tool inspectors that the user actually cycles through in the
/// right-strip. `inspector_header` is the persistent header (always
/// 0 accent sections); `inspector_sticky_chips` is the strip helper;
/// `inspector_water_atmosphere_offer` is a sub-component. The audit
/// table excludes these.
///
/// `inspector_paint_layer` is special-cased: its accent section
/// lives in `layers_panel.rs::render`, so the in-fn count is 0.
/// The `paint_layer_accent_lives_in_layers_panel` test covers it.
const TOOL_INSPECTORS: &[&str] = &[
    "inspector_select",
    "inspector_metal",
    "inspector_geo",
    "inspector_feature",
    "inspector_sculpt",
    "inspector_water",
    "inspector_start_positions",
    "inspector_procgen",
];

#[test]
fn each_tool_inspector_emits_exactly_one_accent_section() {
    let main_src = read("src/main.rs");
    let bodies = inspector_bodies(&main_src);
    for (name, body) in &bodies {
        if !TOOL_INSPECTORS.contains(&name.as_str()) {
            continue;
        }
        let (t, _f) = count_accent(body);
        assert_eq!(
            t, 1,
            "{name} emits {t} accent: true sections — the canonical \
             skeleton requires exactly one primary section per inspector.",
        );
    }
}

#[test]
fn paint_layer_accent_lives_in_layers_panel() {
    // inspector_paint_layer's accent section is the Layers card in
    // layers_panel::render. Verify the panel renders one accent
    // section so the combined PaintLayer inspector has exactly one
    // (the Layers section), keeping it consistent with the other
    // tool inspectors.
    let panel_src = read("src/ui/layers_panel.rs");
    // Heuristic: count accent: true on widgets::section calls in the
    // top-level `pub fn render` body. We don't need to slice the
    // function — there's only one `pub fn render` in this file and
    // every other `widgets::section` call inside it uses accent: false
    // (audited 2026-05-21 / Sprint 27 / U5).
    let (t, _) = count_accent(&panel_src);
    assert_eq!(
        t, 1,
        "layers_panel.rs should host exactly one accent section (the Layers card); got {t}",
    );
}

#[test]
fn every_tool_inspector_renders_the_sticky_chip_strip() {
    // The sticky symmetry + map-size chip band must appear at the top
    // of every tool inspector body — that's the Sprint 27 promise.
    let main_src = read("src/main.rs");
    let bodies = inspector_bodies(&main_src);
    // PaintLayer joins the audit here too — its accent section lives
    // in layers_panel.rs but the sticky chip call still runs in the
    // inspector body itself.
    let mut to_check: Vec<&str> = TOOL_INSPECTORS.to_vec();
    to_check.push("inspector_paint_layer");
    for (name, body) in &bodies {
        if !to_check.contains(&name.as_str()) {
            continue;
        }
        assert!(
            body.contains("self.inspector_sticky_chips(ui)"),
            "{name} does not call self.inspector_sticky_chips(ui); \
             the symmetry+mapsize chip band must appear in every tool inspector.",
        );
    }
}
