# PITFALL §22 — `mapinfo.maxMetal` is a yield scale, not a normalisation cap

The mapinfo field `maxMetal` is the **m/s metal yield** at full
(`1.0`) ground-metal saturation. BAR's `gui_metalspots` widget
computes predicted F4 income as roughly
`spot.worth * incomeMultiplier / 1000` where `spot.worth`
aggregates per-cell ground-metal × `maxMetal` across the spot's
cluster.

## Rule

`MapInfo::bar_default().max_metal = Some(1.0)`. Real BAR maps
cluster in `0.93..=4.11`:

```
jade_empress_1.3      0.99
titanduel_v3          1.26
supreme_isthmus_v2.1  0.93
ravaged_remake_v1.2   1.05
starwatcher_1.0       4.11
```

The lint pass warns if user overrides drop below `0.5` or rise
above `5.0` — outliers are valid (starwatcher_1.0 is balanced
around 4.11) but should be a conscious choice.

## Pre-Sprint-11 default

The editor's pre-2026-05-19 default of `0.02` made a canonical
metal=2.0 spot display as `~0.1` m/s in F4 — 50× too low. Fixed
in the Sprint 11 live-BAR smoke-test hotfix.

## Diagnosis

If F4 metal income reads way too low or way too high for your
spots:

1. Open the F9 mapinfo form, Map tab.
2. Check `maxMetal`. Default is 1.0; should be ~0.5..5.0.
3. Adjust and re-build.

`maxMetal` does not affect the value the editor's metal-spots
inspector shows — that field is the per-spot multiplier (yield
scale × spot-multiplier = m/s in BAR).
