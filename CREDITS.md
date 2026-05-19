# Credits

This editor bundles or depends on the following third-party work. Every
bundled asset and every vendored binary is permissively licensed; the
editor's source itself stays under the licence in `LICENSE`. Maps
authored with the editor inherit no licensing obligations from anything
listed below.

## Starter texture pack — ambientCG

The 16-slot starter texture pack is sourced **entirely from ambientCG**
(<https://ambientcg.com>). Every asset is licensed under the
**Creative Commons CC0 1.0 Universal** dedication:
<https://docs.ambientcg.com/license/> ("All ambientCG assets are
provided under the Creative Commons CC0 1.0 Universal License").

CC0 does not require attribution. We list each asset here as a courtesy
to ambientCG and to make the per-slot provenance auditable for anyone
inspecting the pack.

| # | Slot                  | Biome              | ambientCG asset | URL                                              |
|--:|-----------------------|--------------------|-----------------|--------------------------------------------------|
| 00 | `grass-meadow`        | Earth-Temperate    | Grass002        | <https://ambientcg.com/view?id=Grass002>         |
| 01 | `forest-floor-pine`   | Earth-Temperate    | Ground037       | <https://ambientcg.com/view?id=Ground037>        |
| 02 | `dirt-mud-cracked`    | Earth-Temperate    | Ground042       | <https://ambientcg.com/view?id=Ground042>        |
| 03 | `rocky-outcrop-grey`  | Earth-Temperate    | Rock030         | <https://ambientcg.com/view?id=Rock030>          |
| 04 | `desert-sand-dunes`   | Arid               | Ground027       | <https://ambientcg.com/view?id=Ground027>        |
| 05 | `dry-rock-sandstone`  | Arid               | Rock023         | <https://ambientcg.com/view?id=Rock023>          |
| 06 | `dusty-hardpan-clay`  | Arid               | Ground033       | <https://ambientcg.com/view?id=Ground033>        |
| 07 | `arid-gravel-pebbles` | Arid               | Gravel018       | <https://ambientcg.com/view?id=Gravel018>        |
| 08 | `alpine-snow-powder`  | Snow-Alpine        | Snow004         | <https://ambientcg.com/view?id=Snow004>          |
| 09 | `jagged-ice-frozen`   | Snow-Alpine        | Snow006         | <https://ambientcg.com/view?id=Snow006>          |
| 10 | `cold-bare-rock`      | Snow-Alpine        | Rock029         | <https://ambientcg.com/view?id=Rock029>          |
| 11 | `frozen-permafrost`   | Snow-Alpine        | Ground035       | <https://ambientcg.com/view?id=Ground035>        |
| 12 | `dark-volcanic-lava`  | Alien-Industrial   | Rock035         | <https://ambientcg.com/view?id=Rock035>          |
| 13 | `rusty-metal-plates`  | Alien-Industrial   | Metal009        | <https://ambientcg.com/view?id=Metal009>         |
| 14 | `clean-metal-floor`   | Alien-Industrial   | Metal003        | <https://ambientcg.com/view?id=Metal003>         |
| 15 | `alien-organic-creep` | Alien-Industrial   | Moss001         | <https://ambientcg.com/view?id=Moss001>          |

Pack layout, sourcing policy, and the rationale for selecting each
asset are recorded in `docs/DECISIONS.md` ADR-025 + ADR-027.
The fetch script is `scripts/fetch-textures.sh`.

## Vendored tools

The editor invokes these binaries as sidecars at build time. They are
not redistributed as part of the editor binary itself; they are
downloaded into `tools/` by their respective `scripts/fetch-*.sh`
helpers.

### PyMapConv — Beherith

SMF / SMT compilation goes through **PyMapConv**
(<https://github.com/Beherith/springrts_smf_compiler>) by Peter "Beherith"
Sarkozy. Licensed **CC0-1.0** as of the pinned `v0.6.3` release. The
editor relies on PyMapConv for the entire `.smf` + `.smt` compile path —
without Beherith's tool, the editor's `Build` button would not work.
Thanks. See `docs/DECISIONS.md` ADR-011.

### Compressonator — AMD GPUOpen

BC1 / BC3 / BCn texture compression is performed by AMD's
**Compressonator** (<https://github.com/GPUOpen-Tools/compressonator>).
Licensed **MIT**. PyMapConv invokes `CompressonatorCLI` by name on
Linux; we vendor it under `tools/compressonator/`. See
`docs/DECISIONS.md` ADR-014.

## Reference research

The texture-pack palette and the splat-rendering pipeline owe their
shape to:

- **Beherith's *Advanced SpringRTS Mapping Guide*** — the canonical
  reference for DNTS, normal-map conventions, and the high-pass-
  diffuse-in-alpha workflow that ADR-034 will eventually adopt.
  <https://docs.google.com/document/d/1PL8U2bf-c5HuSVAihdldDTBA5fWKoeHKNb130YDdd-w/edit>
- **Spring RTS engine wiki — Mapdev:splatdetailnormals** —
  <https://springrts.com/wiki/Mapdev:splatdetailnormals>
- **Beyond All Reason map-format reference gist** (burnhamrobertp) —
  the bare-minimum-viable-map reference that the engine scanner
  validates against.
  <https://gist.github.com/burnhamrobertp/97cae4d300e675ca261e661fc58266d1>
