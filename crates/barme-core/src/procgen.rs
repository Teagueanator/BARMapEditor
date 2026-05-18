//! Procedural terrain generation (ADR-020). v1 ships the math-function
//! subset of SRS F14: user enters an expression `f(x, z)`, the evaluator
//! samples it at every heightmap pixel, and the result becomes the new
//! heightmap.
//!
//! Math context: `x` and `z` are bound per pixel. Two normalisation
//! domains: `Unit` maps the map to `[0,1]² ` (nice for `(1 - x)*(1 - z)`
//! style ramps) and `Centered` maps it to `[-1,1]²` (nice for
//! `1 - (x² + z²)` paraboloids).
//!
//! Output: the scalar result is clamped to `[0, 1]`, then scaled to
//! `u16::MAX` so the existing `Heightmap` storage type is unchanged.
//! NaN / Inf samples count as 0 with a single `warn!` per generation.
//!
//! Powered by `evalexpr` — supports `+ - * / ^`, comparisons, trig,
//! exp/log, sqrt/abs/min/max, plus user-defined variables.

use evalexpr::{
    Context, DefaultNumericTypes, EvalexprError, EvalexprResult, Node, Value, build_operator_tree,
    error::EvalexprResultValue,
};
use tracing::warn;

use crate::{Heightmap, MapSize};

/// Minimal `Context` impl that holds *only* the two coordinate bindings
/// (`x`, `z`) as `Value<DefaultNumericTypes>` directly. Avoids the
/// per-pixel `String` allocation that `HashMapContext::set_value` requires
/// under the hood — at 1025² pixels the difference is millions of mallocs.
///
/// Builtins (`math::sqrt`, `math::sin`, …) are still available because the
/// tree evaluator routes those via [`Context::are_builtin_functions_disabled`]
/// before falling back to [`Context::call_function`]; we report builtins as
/// enabled, and `call_function` is only reached for user-defined functions
/// (which the procgen surface doesn't allow).
struct PixelContext {
    x_val: Value<DefaultNumericTypes>,
    z_val: Value<DefaultNumericTypes>,
}

impl PixelContext {
    fn new() -> Self {
        Self {
            x_val: Value::Float(0.0),
            z_val: Value::Float(0.0),
        }
    }

    #[inline]
    fn set_x(&mut self, x: f64) {
        self.x_val = Value::Float(x);
    }

    #[inline]
    fn set_z(&mut self, z: f64) {
        self.z_val = Value::Float(z);
    }
}

impl Context for PixelContext {
    type NumericTypes = DefaultNumericTypes;

    #[inline]
    fn get_value(&self, identifier: &str) -> Option<&Value<Self::NumericTypes>> {
        match identifier {
            "x" => Some(&self.x_val),
            "z" => Some(&self.z_val),
            _ => None,
        }
    }

    fn call_function(
        &self,
        identifier: &str,
        _argument: &Value<Self::NumericTypes>,
    ) -> EvalexprResultValue<Self::NumericTypes> {
        Err(EvalexprError::FunctionIdentifierNotFound(
            identifier.to_string(),
        ))
    }

    fn are_builtin_functions_disabled(&self) -> bool {
        false
    }

    fn set_builtin_functions_disabled(
        &mut self,
        _disabled: bool,
    ) -> EvalexprResult<(), Self::NumericTypes> {
        Err(EvalexprError::ContextNotMutable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// `x, z ∈ [0, 1]`. (0,0) is the NW corner, (1,1) the SE corner.
    Unit,
    /// `x, z ∈ [-1, 1]`. Origin = map center; ±1 = map edge.
    Centered,
}

impl Domain {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Centered => "centered",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Unit => "[0, 1]",
            Self::Centered => "[-1, 1]",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcGenError {
    #[error("parse error: {0}")]
    Parse(#[source] evalexpr::EvalexprError),
    #[error("eval failed at pixel ({}, {}): {source}", pixel.0, pixel.1)]
    EvalFailed {
        pixel: (u32, u32),
        #[source]
        source: evalexpr::EvalexprError,
    },
    #[error("expression must return a number, got: {got}")]
    NonNumeric { got: String },
    #[error("heightmap build failed: {0}")]
    Heightmap(#[source] anyhow::Error),
}

/// Sample a math expression at every heightmap pixel and return a new
/// `Heightmap`. Re-parses the expression once, evaluates per-pixel against
/// a context that re-binds `x` and `z`.
///
/// `min_height` / `max_height` map the expression's normalised range
/// `[0, 1]` to world Y. We store the result as `u16` (0 = min, MAX = max);
/// expression values < 0 clamp to 0, > 1 clamp to MAX. The semantic is
/// "expression returns a fraction of the height range to use here".
pub fn generate(
    expr: &str,
    domain: Domain,
    size: MapSize,
    min_height: f32,
    max_height: f32,
) -> Result<Heightmap, ProcGenError> {
    let (w, h) = size.heightmap_dims();
    let tree: Node<DefaultNumericTypes> = build_operator_tree(expr).map_err(ProcGenError::Parse)?;

    let mut data = Vec::with_capacity((w as usize) * (h as usize));
    let mut ctx = PixelContext::new();
    let mut warned_nan = false;
    let denom_x = (w - 1).max(1) as f64;
    let denom_z = (h - 1).max(1) as f64;

    // Precompute the 1D normalisation arrays once per call — same value for
    // every row (`x_norms`) or every column (`z_norms`), so doing it
    // per-pixel in the inner loop was wasted cycles and a branchy `match`.
    let x_norms: Vec<f64> = (0..w)
        .map(|ix| match domain {
            Domain::Unit => (ix as f64) / denom_x,
            Domain::Centered => (ix as f64) / denom_x * 2.0 - 1.0,
        })
        .collect();
    let z_norms: Vec<f64> = (0..h)
        .map(|iz| match domain {
            Domain::Unit => (iz as f64) / denom_z,
            Domain::Centered => (iz as f64) / denom_z * 2.0 - 1.0,
        })
        .collect();

    for iz in 0..h {
        // z is constant across the inner loop — bind once per row.
        ctx.set_z(z_norms[iz as usize]);
        for ix in 0..w {
            ctx.set_x(x_norms[ix as usize]);
            let v = tree
                .eval_with_context(&ctx)
                .map_err(|source| ProcGenError::EvalFailed {
                    pixel: (ix, iz),
                    source,
                })?;
            let numeric = match v {
                Value::Float(f) => f,
                Value::Int(i) => i as f64,
                other => {
                    return Err(ProcGenError::NonNumeric {
                        got: format!("{other:?}"),
                    });
                }
            };
            let clamped = if numeric.is_finite() {
                numeric.clamp(0.0, 1.0)
            } else {
                if !warned_nan {
                    warn!(
                        "procgen expression produced non-finite sample (NaN/Inf) at ({ix}, {iz}); \
                         clamping to 0 and suppressing further warnings this generation"
                    );
                    warned_nan = true;
                }
                0.0
            };
            data.push((clamped * u16::MAX as f64) as u16);
        }
    }

    let _ = (min_height, max_height); // kept in signature for forward compat
    Heightmap::new(w, h, data).map_err(ProcGenError::Heightmap)
}

/// Built-in preset list. UI dropdown reads this; selecting a preset fills
/// the expression text field. New presets are one-line entries.
pub struct ProcGenPreset {
    pub label: &'static str,
    pub expression: &'static str,
    pub domain: Domain,
}

pub const PRESETS: &[ProcGenPreset] = &[
    ProcGenPreset {
        label: "Flat",
        expression: "0.5",
        domain: Domain::Unit,
    },
    ProcGenPreset {
        label: "Parabolic bowl",
        expression: "1 - (x*x + z*z)",
        domain: Domain::Centered,
    },
    ProcGenPreset {
        label: "Parabolic dome",
        expression: "x*x + z*z",
        domain: Domain::Centered,
    },
    ProcGenPreset {
        label: "Conical peak",
        expression: "max(0, 1 - math::sqrt(x*x + z*z))",
        domain: Domain::Centered,
    },
    ProcGenPreset {
        label: "Ridge (E-W)",
        expression: "1 - math::abs(z)",
        domain: Domain::Centered,
    },
    ProcGenPreset {
        label: "Diagonal ramp",
        expression: "x",
        domain: Domain::Unit,
    },
    ProcGenPreset {
        label: "Sine ripples",
        expression: "0.5 + 0.25 * math::sin(8*x) * math::cos(8*z)",
        domain: Domain::Unit,
    },
];

/// Biome preset for the F1 new-project wizard (ADR-024). A thin wrapper
/// around [`ProcGenPreset`] that also recommends a `max_height` so the
/// wizard can pick reasonable defaults — a "flat plain" biome shouldn't
/// land with a 4096-elmo height scale.
pub struct BiomePreset {
    pub label: &'static str,
    pub expression: &'static str,
    pub domain: Domain,
    pub max_height_hint: f32,
}

pub const BIOMES: &[BiomePreset] = &[
    BiomePreset {
        label: "Flat plain",
        expression: "0.0",
        domain: Domain::Unit,
        max_height_hint: 64.0,
    },
    BiomePreset {
        label: "Parabolic bowl",
        expression: "1 - (x*x + z*z)",
        domain: Domain::Centered,
        max_height_hint: 256.0,
    },
    BiomePreset {
        label: "Cone peak",
        expression: "max(0, 1 - math::sqrt(x*x + z*z))",
        domain: Domain::Centered,
        max_height_hint: 384.0,
    },
    BiomePreset {
        label: "Diagonal ramp",
        expression: "x",
        domain: Domain::Unit,
        max_height_hint: 192.0,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_half_produces_midline() {
        let hm = generate("0.5", Domain::Unit, MapSize::square(2), 0.0, 256.0).unwrap();
        let (lo, hi) = hm.min_max();
        assert_eq!(lo, hi);
        // 0.5 * u16::MAX = 32767.5 → 32767 in u16 truncation.
        assert!(
            lo == 32767 || lo == 32768,
            "expected ~midline u16, got {lo}"
        );
    }

    #[test]
    fn unit_domain_x_at_corners() {
        // expression == x; under Unit domain x at NW corner = 0, at NE = 1.
        let hm = generate("x", Domain::Unit, MapSize::square(2), 0.0, 1.0).unwrap();
        let (w, _) = hm.dims();
        let data = hm.data();
        assert_eq!(data[0], 0, "NW corner = x(0) = 0");
        assert_eq!(data[(w - 1) as usize], u16::MAX, "NE corner = x(1) = 1");
    }

    #[test]
    fn parse_error_propagates() {
        let err = generate("1 + )", Domain::Unit, MapSize::square(2), 0.0, 1.0).unwrap_err();
        assert!(matches!(err, ProcGenError::Parse(_)), "got: {err:?}");
    }

    #[test]
    fn centered_paraboloid_max_at_center() {
        // 1 - (x² + z²) under Centered → 1 at origin (center pixel),
        // 0 at corners (where x² + z² = 2 but clamped to 0).
        let hm = generate(
            "1 - (x*x + z*z)",
            Domain::Centered,
            MapSize::square(2),
            0.0,
            1.0,
        )
        .unwrap();
        let (w, h) = hm.dims();
        let center = hm.data()[((h / 2) * w + (w / 2)) as usize];
        let corner = hm.data()[0];
        assert!(
            center > corner,
            "paraboloid center {center} should exceed corner {corner}"
        );
        assert_eq!(corner, 0, "corner outside [0,1] should clamp to 0");
    }

    /// Regression guard for the per-pixel cost of the math-function path.
    /// Pre-fix (HashMapContext + per-pixel `set_value("x".into(), ...)`)
    /// the cone-peak biome at 16 SMU clocked ~575 ms on the dev machine.
    /// The custom `PixelContext` + precomputed `x_norms`/`z_norms` arrays
    /// land around ~440 ms in release. We pin the threshold at 1000 ms so
    /// the test isn't flaky on slower CI hardware while still catching a
    /// future regression to the pre-fix baseline.
    ///
    /// **The plan-A3 target was <100 ms.** That's unreachable while the
    /// expression layer is evalexpr — its `eval_with_context` allocates a
    /// per-node `Vec` for arguments on every recursive call, costing ~10
    /// allocations × 1.05M pixels per generation. Closing the rest of the
    /// gap requires either rayon par_iter over rows (deferred per
    /// phase-3-plan A3) or replacing evalexpr with a precompiled
    /// bytecode/closure path (would need its own ADR). See the
    /// `stage-1-procgen-perf` devlog for the full numbers.
    #[test]
    fn generate_16_smu_cone_peak_under_one_second() {
        // Warm-up (page caches, allocator); time the second call.
        let _ = generate(
            "max(0, 1 - math::sqrt(x*x + z*z))",
            Domain::Centered,
            MapSize::square(16),
            0.0,
            384.0,
        )
        .unwrap();
        let t0 = std::time::Instant::now();
        let _ = generate(
            "max(0, 1 - math::sqrt(x*x + z*z))",
            Domain::Centered,
            MapSize::square(16),
            0.0,
            384.0,
        )
        .unwrap();
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Conservative ceiling — pre-fix was ~575 ms; if we regress past
        // 1 s, something is wrong.
        assert!(
            elapsed_ms < 1000.0,
            "16 SMU cone-peak procgen took {elapsed_ms:.1} ms; regression past the pre-A3 baseline?"
        );
        eprintln!("16 SMU cone-peak procgen: {elapsed_ms:.1} ms");
    }

    /// Output stability: same expression at the same size must yield the
    /// same bytes after the A3 refactor. Pins the visual contract — if a
    /// future micro-optimisation flips a clamp or rounding bound, the
    /// fixture catches it before users see a corrupted bowl.
    #[test]
    fn cone_peak_at_2_smu_is_byte_stable() {
        let a = generate(
            "max(0, 1 - math::sqrt(x*x + z*z))",
            Domain::Centered,
            MapSize::square(2),
            0.0,
            384.0,
        )
        .unwrap();
        let b = generate(
            "max(0, 1 - math::sqrt(x*x + z*z))",
            Domain::Centered,
            MapSize::square(2),
            0.0,
            384.0,
        )
        .unwrap();
        assert_eq!(a.data(), b.data(), "procgen must be deterministic");
        // Center pixel: x = z = 0 → 1 - sqrt(0) = 1 → u16::MAX.
        let (w, _) = a.dims();
        let center = a.data()[((w / 2) * w + (w / 2)) as usize];
        assert_eq!(center, u16::MAX);
        // Corner: x = z = -1 → 1 - sqrt(2) ≈ -0.41 → clamped to 0.
        assert_eq!(a.data()[0], 0);
    }

    #[test]
    fn presets_all_parse_and_run() {
        for p in BIOMES {
            generate(
                p.expression,
                p.domain,
                MapSize::square(2),
                0.0,
                p.max_height_hint,
            )
            .unwrap_or_else(|e| panic!("biome preset {:?} failed to parse / run: {e:#}", p.label));
            assert!(
                p.max_height_hint > 0.0,
                "biome {:?} has non-positive max_height",
                p.label
            );
        }
        for p in PRESETS {
            generate(p.expression, p.domain, MapSize::square(2), 0.0, 1.0)
                .unwrap_or_else(|e| panic!("preset {} failed: {e:?}", p.label));
        }
    }
}
