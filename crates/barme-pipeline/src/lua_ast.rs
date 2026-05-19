//! Lua-AST serializer (ADR-029).
//!
//! Replaces the line-stitched string formatter that ADR-013 shipped.
//! Mapinfo + the three sidecar files (`map_metal_layout.lua`,
//! `map_startboxes.lua`, `featureplacer/features.lua`) all share this
//! one pretty-printer; per-block builders in sibling modules construct
//! [`LuaValue`] trees and hand them to [`serialize`].
//!
//! ## Style
//!
//! - 2-space indent.
//! - Trailing commas after every table entry (Lua accepts them; they
//!   keep one-line diffs to a single line).
//! - Tables: integer-keyed entries use `[N] = …`; string-keyed use
//!   `key = …` for valid Lua identifiers, `["weird-key"] = …` otherwise.
//! - Sequence (`Seq`) tables omit explicit keys — they emit as
//!   array-style `{ a, b, c }`. Used where ordering carries semantics
//!   but key visibility doesn't (e.g. feature lists once C6 populates
//!   them, or `depend = { "Map Helper v1", "Spring Bitmaps" }`).
//! - String escaping covers `\`, `"`, `\n`, `\r`, `\t`. Anything else
//!   passes through.
//! - Floats always carry a decimal (`130.0`, never `130`) so the
//!   reader can't mistake them for ints. Whole-number floats use
//!   `{:?}` which yields `"130.0"`.
//!
//! ## Determinism
//!
//! The renderer emits keys in the order they're stored in
//! [`LuaValue::Table`]. Per-block builders are responsible for
//! sorting; `sort_table_by_key` is the canonical helper.

/// One key in a Lua table. Strings emit bare when they parse as a Lua
/// identifier, bracketed otherwise. Integers emit as `[N] = …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaKey {
    Str(String),
    Int(i64),
}

impl LuaKey {
    pub fn str(s: impl Into<String>) -> Self {
        LuaKey::Str(s.into())
    }
    pub fn int(n: i64) -> Self {
        LuaKey::Int(n)
    }
}

/// One Lua value. `Seq` is sugar for an array-style table where keys
/// are omitted at emission time (Lua treats them as `1..N`).
#[derive(Debug, Clone, PartialEq)]
pub enum LuaValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Keyed table: keys are emitted explicitly. Builders are
    /// responsible for ordering (typically alphabetic on the
    /// stringified key for diff-friendliness).
    Table(Vec<(LuaKey, LuaValue)>),
    /// Sequence (array-style) table: emits as `{ v1, v2, v3 }`. No
    /// keys are rendered. Ordering is the builder's responsibility.
    Seq(Vec<LuaValue>),
    /// Mixed Lua table — positional sequence entries followed by
    /// keyed entries. Emits as
    /// `{ v1, v2, v3, key1 = w1, key2 = w2, }`. Lua's table model
    /// assigns positional entries to integer keys `1..N` implicitly
    /// and the keyed entries on top; both shapes coexist in one
    /// table.
    ///
    /// Sprint 12 / D6 uses this for the `splatDetailNormalTex`
    /// subtable: four positional DDS paths + an `alpha` boolean
    /// (FINDINGS §1.8 — the engine's preferred form, distinct from
    /// the legacy `splatDetailNormalTex1..4` numbered-keys form
    /// PITFALL §15 calls out as silently shadowed).
    Mixed {
        values: Vec<LuaValue>,
        keyed: Vec<(LuaKey, LuaValue)>,
    },
}

impl LuaValue {
    pub fn str(s: impl Into<String>) -> Self {
        LuaValue::Str(s.into())
    }
    pub fn int<I: Into<i64>>(n: I) -> Self {
        LuaValue::Int(n.into())
    }
    pub fn float(n: f64) -> Self {
        LuaValue::Float(n)
    }
    pub fn bool(b: bool) -> Self {
        LuaValue::Bool(b)
    }
}

/// Sort a keyed table by its string-rendered key — the canonical
/// determinism rule for [`LuaValue::Table`]. Integer keys sort
/// numerically (rendered as `[N]`); string keys sort lexicographically.
/// String < Int as a class boundary, so plain-keyed pairs come first.
pub fn sort_table_by_key(t: &mut [(LuaKey, LuaValue)]) {
    t.sort_by(|(a, _), (b, _)| match (a, b) {
        (LuaKey::Str(x), LuaKey::Str(y)) => x.cmp(y),
        (LuaKey::Int(x), LuaKey::Int(y)) => x.cmp(y),
        (LuaKey::Str(_), LuaKey::Int(_)) => std::cmp::Ordering::Less,
        (LuaKey::Int(_), LuaKey::Str(_)) => std::cmp::Ordering::Greater,
    });
}

/// Render one [`LuaValue`] as Lua source text. Top-level value is
/// emitted with no surrounding `return` — the caller wraps when
/// needed (the mapinfo emitter wraps `local mapinfo = … return mapinfo`;
/// the sidecars wrap `return …`).
pub fn serialize(v: &LuaValue) -> String {
    let mut out = String::new();
    write_value(&mut out, v, 0);
    out
}

fn write_value(out: &mut String, v: &LuaValue, indent: usize) {
    match v {
        LuaValue::Nil => out.push_str("nil"),
        LuaValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        LuaValue::Int(n) => out.push_str(&n.to_string()),
        LuaValue::Float(f) => out.push_str(&format_float(*f)),
        LuaValue::Str(s) => write_string(out, s),
        LuaValue::Table(items) => write_table(out, items, indent),
        LuaValue::Seq(items) => write_seq(out, items, indent),
        LuaValue::Mixed { values, keyed } => write_mixed(out, values, keyed, indent),
    }
}

fn write_table(out: &mut String, items: &[(LuaKey, LuaValue)], indent: usize) {
    if items.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    out.push('\n');
    let inner = "  ".repeat(indent + 1);
    let close = "  ".repeat(indent);
    for (k, v) in items {
        out.push_str(&inner);
        write_key(out, k);
        out.push_str(" = ");
        write_value(out, v, indent + 1);
        out.push(',');
        out.push('\n');
    }
    out.push_str(&close);
    out.push('}');
}

fn write_seq(out: &mut String, items: &[LuaValue], indent: usize) {
    if items.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    out.push('\n');
    let inner = "  ".repeat(indent + 1);
    let close = "  ".repeat(indent);
    for v in items {
        out.push_str(&inner);
        write_value(out, v, indent + 1);
        out.push(',');
        out.push('\n');
    }
    out.push_str(&close);
    out.push('}');
}

/// Render a mixed table: positional `values` first (bare, no keys),
/// then `keyed` entries (each `key = value,`). Empty table when both
/// are empty. Both empty + a single positional value = inline a
/// braced single-element form for diff-friendliness with the keyed-
/// only and seq-only paths.
fn write_mixed(out: &mut String, values: &[LuaValue], keyed: &[(LuaKey, LuaValue)], indent: usize) {
    if values.is_empty() && keyed.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    out.push('\n');
    let inner = "  ".repeat(indent + 1);
    let close = "  ".repeat(indent);
    for v in values {
        out.push_str(&inner);
        write_value(out, v, indent + 1);
        out.push(',');
        out.push('\n');
    }
    for (k, v) in keyed {
        out.push_str(&inner);
        write_key(out, k);
        out.push_str(" = ");
        write_value(out, v, indent + 1);
        out.push(',');
        out.push('\n');
    }
    out.push_str(&close);
    out.push('}');
}

fn write_key(out: &mut String, k: &LuaKey) {
    match k {
        LuaKey::Str(s) if is_lua_identifier(s) => out.push_str(s),
        LuaKey::Str(s) => {
            out.push('[');
            write_string(out, s);
            out.push(']');
        }
        LuaKey::Int(n) => {
            out.push('[');
            out.push_str(&n.to_string());
            out.push(']');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn is_lua_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    !is_lua_keyword(s)
}

fn is_lua_keyword(s: &str) -> bool {
    matches!(
        s,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    )
}

/// Format a float with a guaranteed decimal point so the reader (and
/// `MapInfo.cpp` parser) cannot mistake it for an integer literal.
///
/// The schema stores every mapinfo float as `f32`. When the emitter
/// passes one through `as f64`, the bit pattern is preserved but the
/// shortest-decimal round-trip widens (`0.1f32 → 0.10000000149011612`).
/// We undo that here: if `f` round-trips through `f32` unchanged, emit
/// the shorter f32 representation; otherwise fall back to f64. Whole
/// numbers gain a trailing `.0`.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "0/0".to_string(); // Lua nan literal; mapinfo shouldn't emit nan
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "math.huge".to_string()
        } else {
            "-math.huge".to_string()
        };
    }
    let f32_v = f as f32;
    let s = if f64::from(f32_v) == f {
        format!("{:?}", f32_v)
    } else {
        format!("{:?}", f)
    };
    debug_assert!(s.contains('.') || s.contains('e'));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nil_bool_int_float_str() {
        assert_eq!(serialize(&LuaValue::Nil), "nil");
        assert_eq!(serialize(&LuaValue::Bool(true)), "true");
        assert_eq!(serialize(&LuaValue::Bool(false)), "false");
        assert_eq!(serialize(&LuaValue::Int(42)), "42");
        assert_eq!(serialize(&LuaValue::Int(-17)), "-17");
        assert_eq!(serialize(&LuaValue::Float(1.0)), "1.0");
        assert_eq!(serialize(&LuaValue::Float(130.0)), "130.0");
        assert_eq!(serialize(&LuaValue::Float(0.5)), "0.5");
        assert_eq!(serialize(&LuaValue::str("hi")), r#""hi""#);
    }

    #[test]
    fn string_escapes_backslash_and_quote() {
        let s = LuaValue::str(r#"hello"world"#);
        assert_eq!(serialize(&s), r#""hello\"world""#);
        let s = LuaValue::str(r"path\to\file");
        assert_eq!(serialize(&s), r#""path\\to\\file""#);
    }

    #[test]
    fn string_escapes_newline_tab_cr() {
        let s = LuaValue::str("line1\nline2\rline3\tend");
        assert_eq!(serialize(&s), r#""line1\nline2\rline3\tend""#);
    }

    #[test]
    fn description_with_quotes_and_newlines_round_trips_via_escape() {
        let s = LuaValue::str("Has \"quotes\" and\nnewlines");
        let out = serialize(&s);
        assert_eq!(out, r#""Has \"quotes\" and\nnewlines""#);
    }

    #[test]
    fn identifier_keys_emit_bare() {
        let t = LuaValue::Table(vec![
            (LuaKey::str("name"), LuaValue::str("alpha")),
            (LuaKey::str("modtype"), LuaValue::Int(3)),
        ]);
        let s = serialize(&t);
        assert!(s.contains("name = \"alpha\","), "got:\n{s}");
        assert!(s.contains("modtype = 3,"), "got:\n{s}");
    }

    #[test]
    fn non_identifier_keys_emit_bracketed() {
        // Numbers, hyphens, lua keywords → bracket form.
        let t = LuaValue::Table(vec![
            (LuaKey::str("end"), LuaValue::Int(1)), // keyword
            (LuaKey::str("weird-key"), LuaValue::Int(2)),
            (LuaKey::str("0bad"), LuaValue::Int(3)),
        ]);
        let s = serialize(&t);
        assert!(s.contains(r#"["end"] = 1,"#));
        assert!(s.contains(r#"["weird-key"] = 2,"#));
        assert!(s.contains(r#"["0bad"] = 3,"#));
    }

    #[test]
    fn integer_keys_use_bracket_form() {
        let t = LuaValue::Table(vec![
            (
                LuaKey::int(0),
                LuaValue::Table(vec![(LuaKey::str("x"), LuaValue::Int(100))]),
            ),
            (
                LuaKey::int(1),
                LuaValue::Table(vec![(LuaKey::str("x"), LuaValue::Int(200))]),
            ),
        ]);
        let s = serialize(&t);
        assert!(s.contains("[0] = {"), "got:\n{s}");
        assert!(s.contains("[1] = {"), "got:\n{s}");
        assert!(s.contains("x = 100,"));
        assert!(s.contains("x = 200,"));
    }

    #[test]
    fn empty_table_emits_inline_braces() {
        let t = LuaValue::Table(vec![]);
        assert_eq!(serialize(&t), "{}");
        let s = LuaValue::Seq(vec![]);
        assert_eq!(serialize(&s), "{}");
    }

    #[test]
    fn seq_emits_array_style() {
        let s = LuaValue::Seq(vec![
            LuaValue::str("Map Helper v1"),
            LuaValue::str("Spring Bitmaps"),
        ]);
        let out = serialize(&s);
        // Two lines, two values, no keys.
        assert!(out.contains(r#""Map Helper v1","#));
        assert!(out.contains(r#""Spring Bitmaps","#));
        assert!(!out.contains("="), "seq must not emit keys; got:\n{out}");
    }

    #[test]
    fn nested_table_indents_with_two_spaces() {
        let t = LuaValue::Table(vec![(
            LuaKey::str("smf"),
            LuaValue::Table(vec![(
                LuaKey::str("smtFileName0"),
                LuaValue::str("maps/alpha.smt"),
            )]),
        )]);
        let out = serialize(&t);
        // Outer at indent 0; inner at indent 1 ("  ").
        assert!(out.contains("  smf = {"), "got:\n{out}");
        assert!(out.contains("    smtFileName0 ="), "got:\n{out}");
    }

    #[test]
    fn sort_table_by_key_string_alpha() {
        let mut t = vec![
            (LuaKey::str("c"), LuaValue::Int(3)),
            (LuaKey::str("a"), LuaValue::Int(1)),
            (LuaKey::str("b"), LuaValue::Int(2)),
        ];
        sort_table_by_key(&mut t);
        assert_eq!(t[0].0, LuaKey::str("a"));
        assert_eq!(t[1].0, LuaKey::str("b"));
        assert_eq!(t[2].0, LuaKey::str("c"));
    }

    #[test]
    fn sort_table_by_key_int_numeric() {
        let mut t = vec![
            (LuaKey::int(3), LuaValue::Int(3)),
            (LuaKey::int(0), LuaValue::Int(0)),
            (LuaKey::int(11), LuaValue::Int(11)),
            (LuaKey::int(2), LuaValue::Int(2)),
        ];
        sort_table_by_key(&mut t);
        assert_eq!(t[0].0, LuaKey::int(0));
        assert_eq!(t[1].0, LuaKey::int(2));
        assert_eq!(t[2].0, LuaKey::int(3));
        assert_eq!(t[3].0, LuaKey::int(11));
    }

    #[test]
    fn sort_table_by_key_strings_before_ints() {
        // Mixed-keyed tables exist in BAR mapinfo (e.g. terrainTypes
        // sometimes carries `default = …` plus `[0..N]` entries).
        let mut t = vec![
            (LuaKey::int(0), LuaValue::Int(0)),
            (LuaKey::str("name"), LuaValue::str("X")),
        ];
        sort_table_by_key(&mut t);
        assert_eq!(t[0].0, LuaKey::str("name"));
        assert_eq!(t[1].0, LuaKey::int(0));
    }

    #[test]
    fn float_whole_number_has_trailing_zero() {
        assert_eq!(format_float(130.0), "130.0");
        assert_eq!(format_float(80.0), "80.0");
        assert_eq!(format_float(0.0), "0.0");
        assert_eq!(format_float(-1.0), "-1.0");
    }

    #[test]
    fn float_fractional_round_trips_shortest() {
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(format_float(0.1), "0.1");
        assert_eq!(format_float(0.02), "0.02");
    }

    #[test]
    fn float_e9_emits_scientific_or_decimal_either_is_valid_lua() {
        let s = format_float(1.0e9);
        // Rust {:?} of 1e9 is "1000000000.0" — both forms parse as Lua
        // numbers, so we only require the dot or 'e' is present.
        assert!(s.contains('.') || s.contains('e'), "got: {s}");
    }

    #[test]
    fn float_from_f32_emits_short_decimal_not_widened() {
        // The schema stores f32 everywhere. After `as f64` the bit
        // pattern preserves but Rust's f64 {:?} would print
        // "0.10000000149011612". The emitter must recognise the f32
        // round-trip and emit "0.1".
        let f = 0.1f32 as f64;
        assert_eq!(format_float(f), "0.1");
        let f = 0.02f32 as f64;
        assert_eq!(format_float(f), "0.02");
    }

    /// D6 (Sprint 12): `LuaValue::Mixed` renders positional entries
    /// first (bare), then keyed entries. Matches FINDINGS §1.8's
    /// modern `splatDetailNormalTex` form: `{ "a.dds", "b.dds",
    /// alpha = false, }`.
    #[test]
    fn mixed_table_emits_positional_then_keyed() {
        let m = LuaValue::Mixed {
            values: vec![LuaValue::str("a.dds"), LuaValue::str("b.dds")],
            keyed: vec![(LuaKey::str("alpha"), LuaValue::Bool(false))],
        };
        let s = serialize(&m);
        // Positional entries come before the keyed `alpha` field.
        let pos_a = s.find(r#""a.dds""#).unwrap();
        let pos_b = s.find(r#""b.dds""#).unwrap();
        let pos_alpha = s.find("alpha = false").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_alpha);
        // Positional entries DO NOT carry a key prefix.
        assert!(
            !s.contains(r#"[1] = "a.dds""#),
            "positional entry must not emit a key prefix; got:\n{s}"
        );
    }

    /// `Mixed` with both vecs empty renders as `{}`.
    #[test]
    fn mixed_table_empty_emits_inline_braces() {
        let m = LuaValue::Mixed {
            values: vec![],
            keyed: vec![],
        };
        assert_eq!(serialize(&m), "{}");
    }

    /// `Mixed` with only positional entries collapses to the same
    /// rendered text as a `Seq` (modulo trivial differences) — both
    /// produce the bare positional form. Pin the equivalence on the
    /// minimal four-string DDS case.
    #[test]
    fn mixed_table_with_no_keyed_matches_seq_emission() {
        let strs = vec![
            LuaValue::str("a.dds"),
            LuaValue::str("b.dds"),
            LuaValue::str("c.dds"),
        ];
        let mixed = LuaValue::Mixed {
            values: strs.clone(),
            keyed: vec![],
        };
        let seq = LuaValue::Seq(strs);
        assert_eq!(
            serialize(&mixed),
            serialize(&seq),
            "Mixed with empty keyed should match Seq exactly"
        );
    }

    /// `Mixed` nests with proper indentation under a parent table.
    #[test]
    fn mixed_table_indents_under_parent_table() {
        let t = LuaValue::Table(vec![(
            LuaKey::str("resources"),
            LuaValue::Table(vec![(
                LuaKey::str("splatDetailNormalTex"),
                LuaValue::Mixed {
                    values: vec![LuaValue::str("a.dds"), LuaValue::str("b.dds")],
                    keyed: vec![(LuaKey::str("alpha"), LuaValue::Bool(true))],
                },
            )]),
        )]);
        let s = serialize(&t);
        assert!(s.contains("splatDetailNormalTex = {"), "got:\n{s}");
        assert!(s.contains(r#""a.dds","#), "got:\n{s}");
        assert!(s.contains("alpha = true,"), "got:\n{s}");
    }
}
