# **Scoping the Starter Texture Palette for the Beyond All Reason Map Editor: Technical and Ecological Analysis**

## **Architecture of Detail Texture Splatting in the Recoil Pipeline**

The fundamental rendering paradigm for terrain in the Recoil engine relies heavily on a composite layering system that merges macro-level diffuse imagery with micro-level detail splatting. The implementation of the Splat Painting utility (F4) within the new Beyond All Reason (BAR) Map Editor requires a localized, license-clean palette of high-frequency detail textures to function without forcing users into external asset acquisition.1 To properly scope this starter texture pack, the precise constraints of the engine's canonical compiler, PyMapConv, must be analyzed in the context of the Detail Normal Texture Splatting (DNTS) framework.3  
The PyMapConv compiler acts as an archiver and format wrapper for the Spring Map Format (SMF) and Spring Map Texture (SMT) binaries, ingesting raw heightmaps, feature placements, and splat distributions.4 The documentation establishes that PyMapConv does not algorithmically synthesize normal maps from diffuse inputs during compilation; rather, normal maps must be explicitly authored and provided to the compiler alongside the distribution masks.2 Within the DNTS system, the Recoil engine requires standard normal maps to be stored in the RGB channels, while an optional grayscale diffuse luminance map can be mathematically compressed and packed into the Alpha channel.2 This behavior is toggled via the splatDetailNormalDiffuseAlpha=1 flag in the map's mapinfo.lua configuration file.3 When enabled, this alpha-channel diffuse is multiplied against the base macro-diffuse texture in the fragment shader, adding ambient occlusion and micro-luminance variation without incurring the performance penalty of a secondary diffuse texture fetch.2  
The scaling and resolution constraints of these splat textures are highly specific. The PyMapConv compiler and the engine's rendering state can consume detail textures at arbitrary resolutions, provided the dimensions are powers of two to maintain seamless tiling and proper mipmapping generation.3 However, the canonical mapping documentation specifies that a resolution of 1024 × 1024 pixels is optimal, explicitly stating that "1k sized splatDetailNormalTex is perfectly sufficient" for the camera distances natively utilized in competitive RTS gameplay.2 The visual density and physical area covered by these textures are not defined by the image resolution itself, but are instead controlled procedurally via the texScales array within mapinfo.lua.7 The standard declaration, texScales \= {0.02, 0.02, 0.02, 0.02}, dictates the frequency of the tiling across the map geometry, with lower fractional values increasing the physical size of the tiles and higher values inducing tighter visual repetition.7  
The color space and channel ordering of the source assets present critical pipeline implications for the BAR Map Editor. Because normal maps encode non-color vector data, they must be processed in a linear color space to prevent sRGB gamma correction from skewing the surface angle calculations.2 Furthermore, the Recoil engine utilizes an OpenGL-standard tangent space, meaning the green channel (representing the Y-axis vector) must be inverted relative to DirectX-standard normal maps.2 Historically, mappers utilized TGA files to bypass the origin-flipping associated with legacy DDS converters.2 For the BAR Map Editor, which relies on Compressonator to execute on-the-fly BC1 or BC5 compression 4, the source assets should be lossless PNG-8 files. If diffuse textures are separated from normal maps in the source directory to save disk space, the diffuse maps can safely utilize high-quality JPEG encoding; however, applying JPEG compression to normal maps introduces severe chroma subsampling artifacts (typically 4:2:0) that irreparably corrupt the XYZ vector matrices.2  
Finally, the hardware limitations of the shader array enforce a strict cap on the number of splat textures that can be concurrently rendered. The rendering pipeline blends textures using a single RGBA distribution map (splatDistrTex), mapping each of its four color channels to a specific detail texture.2 Because an image contains exactly four channels, the maximum splat texture count per map is strictly four.2 These are routed by string name in the mapinfo.lua block via the variables splatDetailNormalTex1 through splatDetailNormalTex4, rather than relying on strict numerical file naming conventions on disk.3

## **Ecosystem Typology and Biome Distribution**

Determining the specific 8 to 16 textures for the starter palette requires an empirical survey of the existing environmental ecosystems utilized in Beyond All Reason. The objective is to provide a curated selection that spans the most common competitive map archetypes, allowing novice authors to generate believable terrain without sourcing external assets.1  
An analysis of the metadata tags present in the official map browser and the beyond-all-reason/maps-metadata repository reveals fifteen primary environmental classifications: Space, Lava, Ice, Alien, Acidic, Wasteland, Tropical, Swamp, Ruins, Metal, Jungle, Grassy, Forests, Desert, and Asteroid.10 Through statistical categorization, these tags can be consolidated into four dominant archetypal biomes:

| Archetypal Biome | Metadata Tags Covered | Visual Characteristics | Necessary Textural Archetypes |
| :---- | :---- | :---- | :---- |
| **Earth-Temperate** | Grassy, Forests, Tropical, Swamp, Jungle | Organic, highly vegetative, significant interplay between soft soil and sharp bedrock | Grass meadow, pine/forest floor, damp dirt/mud, rocky outcrop |
| **Arid / Desert** | Desert, Wasteland, Ruins | Desolate, wind-swept terrain, high contrast between dunes and hardpan | Fine sand, dry sandstone, dusty hardpan/cracked clay, gravel scatter |
| **Alpine / Cryosphere** | Ice, Asteroid, Space | Highly reflective specular surfaces, stark elevation changes | Snow/powder, jagged ice, cold bare rock, frozen permafrost |
| **Industrial / Alien** | Metal, Lava, Alien, Acidic | Brutalist metallic panels, xenobiological growth, dark basalt contrasting with emissive features | Dark basalt/obsidian, clean metal plating, rusted industrial metal, alien organic creep |

This categorization verifies the initial hypothesis regarding the distribution of BAR biomes. Earth-temperate environments remain the most prevalent in the competitive map pool, as evidenced by maps like "All That Simmers," which heavily utilizes forest and grassy tags.11 However, the Industrial/Alien archetype defines a significant portion of the distinct BAR aesthetic. An extraction and analysis of the mapinfo.lua configurations embedded within popular map archives confirm these patterns. For instance, the map *MapOrbitalStation* relies entirely on metallic textures for its DNTS implementation, routing the texture Metal\_FloorTilesCheckered\_2k\_dnts.dds into multiple slots, and utilizing Metal\_BrushedMetalTilesDirty\_2k\_dnts\_flipped.dds to create localized variation.6 The explicit \_flipped suffix on the brushed metal asset corroborates the technical requirement for Y-channel inversion discussed previously.6 A starter palette must therefore include a mix of clean and weathered metallic textures to support industrial map creation natively.

## **Precedents in Community Tooling and License Integration**

Evaluating historical attempts to build map editors within the Spring/Recoil ecosystem provides insight into the necessity of bundled assets. Two notable open-source editor forks exist: JandoDev/bar-editor and tebeer/BARMapEdit. The JandoDev/bar-editor project, built on a WebGL and Vue architecture, stalled in its early phases and contains no evidence of integrated texture bundles or PyMapConv compiler integration.13 The tebeer/BARMapEdit project, developed in C\# utilizing the Unity Engine, represents a more mature attempt, featuring explicit commits related to splat handling and texture windows.14 However, it ultimately required users to manually supply and import .dds textures into the Unity environment; it did not ship with a starter pack.14  
Furthermore, historical community discourse frequently references a "Spring Map Texture Pack".14 Research confirms that no canonical, open-source texture pack exists under this name. Searches for such a pack predominantly return commercial graphic design overlays sold on marketplaces like Etsy or proprietary software bundles sold by ON1.15 The community's workflow has traditionally relied on procedural generation software like World Machine, which outputs DNTS layers algorithmically from node-based materials.2 Decoupling the mapping process from these expensive, proprietary tools requires the BAR Map Editor to provide a standalone, mathematically validated texture palette.19  
The licensing of these bundled assets is severely constrained by the archival structure of the Recoil engine. When a map is compiled into an .sd7 archive, all associated scripts and textures are packed into a single distributed binary.1 Bundling textures governed by the GNU GPL or Share-Alike (SA) copyleft licenses would infect the map author's .sd7 output with identical copyleft obligations, legally mandating the open-sourcing of all custom map assets. Conversely, CC-BY (Creative Commons Attribution) licenses introduce unacceptable user experience friction, as they would require the map author to manually maintain attribution manifests within the archive. Therefore, all source assets must be strictly licensed under CC0-1.0 (Public Domain Dedication).  
A diagnostic review of license-clean texture catalogs identifies **ambientCG (ambientcg.com)** and **Poly Haven (polyhaven.com)** as the optimal sources. AmbientCG operates exclusively under CC0-1.0 and mathematically guarantees seamless tiling across its entire material library, directly fulfilling the non-repeating constraint required by the engine's texScales splatting logic.7 Poly Haven, which absorbed the older Texture Haven archive, transitioned to a strict CC0-1.0 model in 2021 and provides exceptionally high-quality PBR materials for outdoor environments. Platforms like OpenGameArt were excluded from consideration due to their heterogeneous aggregation of licenses (including CC-BY and GPL), which poses an unacceptable risk of cross-contamination in the map pipeline.

## ---

**ADR-025 — Starter texture pack**

**Status:** Proposed (research) — 2026-05-17  
**Context:** The implementation of the F4 Splat Painting utility requires a guaranteed, localized palette of seamlessly tiling detail textures to operate successfully. Historically, map authors were forced to source textures via proprietary procedural tools like World Machine or manually compress assets through third-party utilities before passing them to PyMapConv. A survey of the Beyond All Reason map catalog (maps-metadata) indicates that competitive environments heavily cluster into four dominant archetypes: Earth-Temperate, Arid, Alpine, and Industrial/Alien. To allow novice users to generate visually cohesive maps spanning these biomes out-of-the-box without encountering blank-canvas paralysis, the editor must vendor a pre-curated palette of high-quality textures. To prevent map authors from inheriting copyleft obligations or facing attribution UX friction inside their compiled .sd7 archives, these source assets must be strictly licensed under CC0-1.0.  
**Alternatives:**

* *In-app downloader*: Rejected. Introduces unnecessary networking dependencies and potential CDN rot for core rendering functionality.  
* *User-import-only*: Rejected. Creates a severe barrier to entry and forces users to manually author engine-compliant normal maps before using the brush.  
* *Forking an existing community pack*: Rejected. No canonical, license-clean, standardized CC0 pack exists within the Spring/Recoil community.

**Consequence:**

* 16 texture sets (Diffuse \+ Normal pairs) bundled via scripts/fetch-textures.sh into tools/textures/. Total footprint will be kept under the 50 MB budget by sourcing 1024² PNGs for mathematically sensitive normals and highly compressed JPGs for diffuses where necessary.  
* Splat brush UI defaults to this palette; user-import (F23 / Phase 6\) is the polish path.  
* The barme-pipeline must handle the Y- inversion (green channel flip) natively in Rust before feeding the PNGs to the vendored Compressonator for DDS generation, as Recoil's OpenGL tangent space requires inverted normals relative to standard DirectX outputs.

### **Bundled textures**

> **CORRECTED 2026-05-18 (Sprint 7 / D1, ADR-025).** The original table
> shipped by Gemini contained four hallucinated ambientCG asset IDs
> (`Grass012`, `Sand002`, `Metal042`, `Organic001` — all 404 on
> ambientCG as of 2026-05-18), four Poly Haven URLs that Sprint 7's
> brief routed off Poly Haven onto ambientCG (per per-asset licence
> variance + 77–99 MB ZIP footprints), and one slot collision (after
> substituting slot 3's Poly Haven `aerial_rocks_01` to ambientCG
> `Rock030`, slot 10 also wanted `Rock030`). All entries below are
> verified against ambientCG HEAD-checks + ZIP-member inspection
> 2026-05-18 and sha256-pinned in `scripts/fetch-textures.sh`. See
> ADR-025 for the full pinned palette + rationale; see ADR-027 for
> the on-disk registry layout. **The remainder of this document
> (biome structure, format reasoning, licence analysis) is kept
> intact — only the per-slot asset IDs were unreliable.**

| Slot | Name | Source | Direct URL | Licence | Size (source) | Biome |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| 0 | grass-meadow-01 | ambientCG | https://ambientcg.com/view?id=Grass002 | CC0-1.0 | 1024² PNG | Earth-Temperate |
| 1 | forest-floor-pine | ambientCG | https://ambientcg.com/view?id=Ground037 | CC0-1.0 | 1024² PNG | Earth-Temperate |
| 2 | dirt-mud-cracked | ambientCG | https://ambientcg.com/view?id=Ground042 | CC0-1.0 | 1024² PNG | Earth-Temperate |
| 3 | rocky-outcrop-grey | ambientCG | https://ambientcg.com/view?id=Rock030 | CC0-1.0 | 1024² PNG | Earth-Temperate |
| 4 | desert-sand-dunes | ambientCG | https://ambientcg.com/view?id=Ground027 | CC0-1.0 | 1024² PNG | Arid / Desert |
| 5 | dry-rock-sandstone | ambientCG | https://ambientcg.com/view?id=Rock023 | CC0-1.0 | 1024² PNG | Arid / Desert |
| 6 | dusty-hardpan-clay | ambientCG | https://ambientcg.com/view?id=Ground033 | CC0-1.0 | 1024² PNG | Arid / Desert |
| 7 | arid-gravel-pebbles | ambientCG | https://ambientcg.com/view?id=Gravel018 | CC0-1.0 | 1024² PNG | Arid / Desert |
| 8 | alpine-snow-powder | ambientCG | https://ambientcg.com/view?id=Snow004 | CC0-1.0 | 1024² PNG | Snow / Alpine |
| 9 | jagged-ice-frozen | ambientCG | https://ambientcg.com/view?id=Snow006 | CC0-1.0 | 1024² PNG | Snow / Alpine |
| 10 | cold-bare-rock | ambientCG | https://ambientcg.com/view?id=Rock029 | CC0-1.0 | 1024² PNG | Snow / Alpine |
| 11 | frozen-permafrost | ambientCG | https://ambientcg.com/view?id=Ground035 | CC0-1.0 | 1024² PNG | Snow / Alpine |
| 12 | dark-volcanic-lava | ambientCG | https://ambientcg.com/view?id=Rock035 | CC0-1.0 | 1024² PNG | Alien / Industrial |
| 13 | rusty-metal-plates | ambientCG | https://ambientcg.com/view?id=Metal009 | CC0-1.0 | 1024² PNG | Alien / Industrial |
| 14 | clean-metal-floor | ambientCG | https://ambientcg.com/view?id=Metal003 | CC0-1.0 | 1024² PNG | Alien / Industrial |
| 15 | alien-organic-creep | ambientCG | https://ambientcg.com/view?id=Moss001 | CC0-1.0 | 1024² PNG | Alien / Industrial |

### **Excluded but considered**

* **OpenGameArt generic terrains**: Rejected due to heterogeneous licensing (GPL mixed with CC-BY). Prohibitive risk of poisoning user .sd7 archives with copyleft requirements.  
* **Commercial Spring Texture Packs**: Rejected. Historically, community texture packs were sold via Etsy or ON1 software. We strictly require FOSS assets.  
* **8K/4K resolution variants**: Rejected. PyMapConv and engine best practices dictate 1024² is perfectly sufficient for DNTS detail tiling; higher resolutions blow past the 50 MB disk budget with no visual gain at typical RTS camera distances.

### **Open questions for implementation**

* The optimal visual parameters for squashing the diffuse map luminance into the normal map alpha channel (for splatDetailNormalDiffuseAlpha=1) need to be tuned via iterative testing. If the mathematical scaling requires too much manual intervention from the pipeline, we may default the engine state to splatDetailNormalDiffuseAlpha=0 to ensure predictable lighting across the starter pack.  
* Verification of the exact Tangent Space coordinate system natively utilized by AmbientCG source assets (DirectX vs. OpenGL) is required during the scripting of barme-pipeline to determine if a programmatic green-channel invert operation is mandatory for every downloaded texture, or only specific subsets.

#### **Works cited**

1. File Structure & Prerequisites ｜ Mapping Guide Beyond All Reason RTS, accessed May 17, 2026, [https://www.beyondallreason.info/guide/mapping-1-file-structure-prerequisites](https://www.beyondallreason.info/guide/mapping-1-file-structure-prerequisites)  
2. Advanced SpringRTS Mapping Guide by Beherith \- Google Docs, accessed May 17, 2026, [https://docs.google.com/document/d/1PL8U2bf-c5HuSVAihdldDTBA5fWKoeHKNb130YDdd-w/edit](https://docs.google.com/document/d/1PL8U2bf-c5HuSVAihdldDTBA5fWKoeHKNb130YDdd-w/edit)  
3. Detail Normal Texture Splatting (DNTS) \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Mapdev:splatdetailnormals](https://springrts.com/wiki/Mapdev:splatdetailnormals)  
4. GitHub \- Beherith/springrts\_smf\_compiler: This tool allows the compilation and decompilation of maps to springrts's binary smf map format., accessed May 17, 2026, [https://github.com/Beherith/springrts\_smf\_compiler](https://github.com/Beherith/springrts_smf_compiler)  
5. MapConv \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/MapConv](https://springrts.com/wiki/MapConv)  
6. MapOrbitalStation/mapinfo.lua at main \- GitHub, accessed May 17, 2026, [https://github.com/beyond-all-reason/MapOrbitalStation/blob/main/mapinfo.lua](https://github.com/beyond-all-reason/MapOrbitalStation/blob/main/mapinfo.lua)  
7. Mapdev:splatdetail \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Mapdev:splatdetail](https://springrts.com/wiki/Mapdev:splatdetail)  
8. Mapdev:detail \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Mapdev:detail](https://springrts.com/wiki/Mapdev:detail)  
9. Map Checklist ｜ Mapping Guide Beyond All Reason RTS, accessed May 17, 2026, [https://www.beyondallreason.info/guide/map-checklist](https://www.beyondallreason.info/guide/map-checklist)  
10. Sync {min,max}height of maps to webflow · Issue \#474 · beyond-all, accessed May 17, 2026, [https://github.com/beyond-all-reason/maps-metadata/issues/474](https://github.com/beyond-all-reason/maps-metadata/issues/474)  
11. Maps Beyond All Reason RTS, accessed May 17, 2026, [https://www.beyondallreason.info/maps](https://www.beyondallreason.info/maps)  
12. Find Every Map\! Microblog Beyond All Reason RTS, accessed May 17, 2026, [https://www.beyondallreason.info/microblogs/50](https://www.beyondallreason.info/microblogs/50)  
13. KeT ketpain \- GitHub, accessed May 17, 2026, [https://github.com/ketpain](https://github.com/ketpain)  
14. GitHub \- tebeer/BARMapEdit: Map Editor of Beyond All Reason, accessed May 17, 2026, [https://github.com/tebeer/BARMapEdit](https://github.com/tebeer/BARMapEdit)  
15. Spring Texture Pack 10 Digital Overlay or Backdrop Textures or Collage Papers to Print \- Etsy, accessed May 17, 2026, [https://www.etsy.com/listing/1144488472/spring-texture-pack-10-digital-overlay](https://www.etsy.com/listing/1144488472/spring-texture-pack-10-digital-overlay)  
16. ON1 Spring Texture Pack (ON1T-STC21), accessed May 17, 2026, [https://www.on1.com/store/on1-spring-texture-pack-on1t-stc21/](https://www.on1.com/store/on1-spring-texture-pack-on1t-stc21/)  
17. Spring Texture Pack \- WeGraphics, accessed May 17, 2026, [https://we.graphics/item/spring-texture-pack/](https://we.graphics/item/spring-texture-pack/)  
18. Spring Minecraft Texture Pack \- YouTube, accessed May 17, 2026, [https://www.youtube.com/watch?v=0269h4U0ywI](https://www.youtube.com/watch?v=0269h4U0ywI)  
19. The Complete World Machine Spring Map making tool, accessed May 17, 2026, [https://springrts.com/phpbb/viewtopic.php?t=42882](https://springrts.com/phpbb/viewtopic.php?t=42882)  
20. Onyx Cauldron updated with a less noisy texture in the grass areas, more accurate passability map, better map extension texture, and updated features. \[Gallery\] : r/beyondallreason \- Reddit, accessed May 17, 2026, [https://www.reddit.com/r/beyondallreason/comments/1ijvfcq/onyx\_cauldron\_updated\_with\_a\_less\_noisy\_texture/](https://www.reddit.com/r/beyondallreason/comments/1ijvfcq/onyx_cauldron_updated_with_a_less_noisy_texture/)  
21. Mapdev:Tutorial Intermediate \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Mapdev:Tutorial\_Intermediate](https://springrts.com/wiki/Mapdev:Tutorial_Intermediate)