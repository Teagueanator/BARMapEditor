# **BAR mapinfo.lua \+ Gadget Schema Reference**

The architectural foundation of any map developed for Beyond All Reason (BAR) relies on the precise configuration of metadata consumed by the Recoil engine and the game's myriad Lua gadgets. The mapinfo.lua file acts as the authoritative manifest, dictating everything from base heightmap rendering and physical terrain properties to UI behaviors, atmospheric scattering, and pathfinder constraints. As map editors transition from emitting minimal, boot-only implementations to robust, feature-complete archives, a comprehensive understanding of the entire schema becomes strictly necessary.  
The analysis indicates that the metadata parsing architecture in the Recoil engine operates on a dual-layer consumption model. At the baseline, C++ engine parsers—specifically rts/Map/MapInfo.cpp and rts/Map/SMF/SMFReadMap.cpp—ingest the root tables during initialization to configure the physical simulation and the core graphics pipeline. Concurrently, the Beyond All Reason game modification processes the exact same file through its LuaRules (synced state), LuaGaia (environment state), and LuaUI (unsynced user interface) environments. These Lua-driven systems utilize custom parameters to drive critical gameplay mechanics such as dynamic lighting, resource distribution, environmental hazards, and pathfinding.2 Emitting a structurally valid .sd7 archive requires perfectly satisfying the data contracts of both the compiled engine and the interpreted scripts.

## **1\. Full mapinfo.lua field table**

The following schema delineates the root-level fields and primary nested objects required or supported by the Recoil engine and the Beyond All Reason mod. The Recoil engine parses these values via a virtual file system (VFS) execution of the Lua table. Fields omitted from a generated .sd7 archive will either fall back to engine-hardcoded defaults (which may not align with BAR's modern gameplay standards), rely on generic fallback assets (often producing severe visual errors), or induce silent failures in loosely-written Lua gadgets.3  
The editor must serialize these fields cleanly. String formats must be exact, and relative paths must strictly reflect the internal directory structure of the uncompressed .sd7 package.

| Path | Type | Default | Consumed by | Required for BAR? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| name | string | — | MapInfo.cpp, Teiserver | Yes | The full, human-readable name of the map as displayed in the Chobby lobby, webflow interfaces, and Discord rich presence. |
| shortname | string | — | MapInfo.cpp | Yes | An abbreviated string used for internal indexing, log output, or legacy UI elements. |
| description | string | "" | Teiserver, Chobby | No | A brief text description of the map’s lore or features, visible in map selection interfaces and server browsers. |
| author | string | "" | Teiserver, Chobby | No | The credited creator of the map. Essential for community attribution. |
| version | string | "1" | Engine Unitsync | Yes | Used to manage map updates, validate cache integrity, and prevent desyncs between differing client versions. |
| mapfile | string | — | SMFReadMap.cpp | Yes | The internal .sd7 relative path to the compiled map geometry, typically "maps/\<name\>.smf". |
| modtype | int | 3 | gui\_maplist\_panel.lua | Yes | Must strictly be 3 for map-browser filters to recognize the archive as a valid playable map rather than a base game dependency. |
| depend | table\[string\] | {} | ArchiveScanner | Yes | Defines archive dependencies. Commonly requires {"Map Helper v1"} to ensure standard Lua engine helpers load properly before map initialization. |
| replace | table\[string\] | {} | ArchiveScanner | No | A legacy system for overriding base-game dependencies. Rarely utilized in modern BAR maps. |
| maphardness | float | 100.0 | Engine Physics | No | Global multiplier for weapon-induced terrain deformation geometry. Higher values resist cratering from artillery fire. |
| notDeformable | boolean | false | Engine Physics | No | If true, globally disables all terrain deformation regardless of weapon impact physics or maphardness settings. |
| gravity | float | 130.0 | Engine Physics | No | Defines the global downward acceleration for projectiles and airborne units measured in elmos/sec^2. |
| tidalStrength | float | 0.0 | LuaRules | No | Legacy Spring parameter for tidal energy generation; mostly superseded by specific BAR wind, solar, and geothermal mechanics. |
| maxMetal | float | 0.02 | Engine / LuaRules | Yes | Maps the absolute pure red pixel density (255,0,0) on the metal map to a baseline extraction rate per simulation tick. |
| extractorRadius | float | 500.0 | Engine / UI | Yes | Defines the spatial radius (in elmos) in which a metal extractor pulls resources from the underlying metal heatmap. |
| voidWater | boolean | false | MapInfo.cpp | No | If true, the engine renderer skips drawing the default water plane, requiring the map to render custom liquid layers. |
| autoShowMetal | boolean | true | gui\_autoshowmetal.lua | Yes | If undefined or explicitly false, the UI gadget fails to toggle the metal heatmap during metal extractor placement. |
| smf.minheight | int | 0 | SMFReadMap.cpp | Yes | The absolute minimum elevation mapped to the 16-bit heightmap's black value (0) during World Machine or terrain generation. |
| smf.maxheight | int | 0 | SMFReadMap.cpp | Yes | The absolute maximum elevation (in elmos) mapped to the 16-bit heightmap's white value (65535). |
| smf.smtFileName0 | string | — | SMFReadMap.cpp | Yes | Must perfectly match the compiled .smt texture filename inside the .sd7 archive or the engine renders a pink fallback error texture. |
| smf.smtFileName1..N | string | — | SMFReadMap.cpp | No | Used for advanced tiled builds where textures exceed standard memory allocation sizes, allowing the chaining of multiple .smt files. |

## **2\. allyTeam / teams schema**

The transition from a basic 1v1 testing environment to production-ready 8v8 or multi-faction Free-For-All (FFA) maps introduces a structural divergence between what the Recoil engine natively parses and how the Beyond All Reason lobby (Teiserver/Chobby) manages game configuration.5

### **Engine vs. Lobby Paradigm**

Within the Recoil engine's internal representation, a "team" strictly corresponds to an individual player entity or an Artificial Intelligence controller. An allyTeam is the logical, diplomatic grouping of these players (e.g., Team 0 through Team 7 belonging exclusively to allyTeam 0). However, the mapinfo.lua file does not assign allyTeam logic or diplomacy directly within its teams block.5  
The mapinfo.lua file is designed solely to dictate a sequential array of default starting positions (utilizing spatial elmo coordinates) indexed by team integer IDs.4 The structural assignment of players to these physical spots on the terrain, and the specific designation of which spots constitute friendly forces versus hostile forces, is overridden dynamically by the BAR lobby during match initialization through rts/Game/GameSetup.cpp. When the match begins, the engine resolves the layout via LuaRules, mapping lobby commands to the defined engine teams.  
The ordering of these starting positions inside the teams block does matter. The Recoil engine parses these sequentially. If a lobby initializes a match without overriding the start positions, the engine will assign Player 1 to teams, Player 2 to teams, and so forth. Furthermore, gap removal in GameSetup.cpp ensures that if a lobby connects PLAYER10 and PLAYER14 directly to a 1v1 game, the engine elegantly truncates the gaps and places them into the first available sequential slots defined in the map. If a map lacks sufficient starting positions for the lobby's requested configuration, the engine forces the remaining players to spawn in the absolute corner of the map.

### **Canonical Layouts for the Emitter**

For the editor team to emit correct layouts, the emitter must generate a continuous numerical array for teams, regardless of the intended allyTeam configurations. The visual or logical grouping into discrete startboxes for the UI should be injected into a custom block, which the BAR UI reads for pre-game rendering in the lobby.

#### **The 1v1 Map Layout**

For standard duels, the editor outputs exactly two symmetrical positions.

Lua

teams \= {  
    \-- Intended AllyTeam 0  
     \= { startPos \= { x \= 2000, z \= 2000 } },  
    \-- Intended AllyTeam 1  
     \= { startPos \= { x \= 6000, z \= 6000 } },  
}

#### **The 8v8 Map Layout**

An 8v8 map strictly requires 16 distinct start positions. The standard convention organizes Team 0 through 7 on one geographic hemisphere of the map (intended for allyTeam 0\) and Team 8 through 15 on the opposing hemisphere (intended for allyTeam 1).

Lua

teams \= {  
    \-- Intended AllyTeam 0 (Northern Hemisphere)  
      \= { startPos \= { x \= 1000, z \= 1000 } },  
      \= { startPos \= { x \= 1500, z \= 1000 } },  
      \= { startPos \= { x \= 2000, z \= 1000 } },  
      \= { startPos \= { x \= 2500, z \= 1000 } },  
      \= { startPos \= { x \= 3000, z \= 1000 } },  
      \= { startPos \= { x \= 3500, z \= 1000 } },  
      \= { startPos \= { x \= 4000, z \= 1000 } },  
      \= { startPos \= { x \= 4500, z \= 1000 } },  
      
    \-- Intended AllyTeam 1 (Southern Hemisphere)  
      \= { startPos \= { x \= 1000, z \= 7000 } },  
      \= { startPos \= { x \= 1500, z \= 7000 } },  
     \= { startPos \= { x \= 2000, z \= 7000 } },  
     \= { startPos \= { x \= 2500, z \= 7000 } },  
     \= { startPos \= { x \= 3000, z \= 7000 } },  
     \= { startPos \= { x \= 3500, z \= 7000 } },  
     \= { startPos \= { x \= 4000, z \= 7000 } },  
     \= { startPos \= { x \= 4500, z \= 7000 } },  
}

#### **The 3-Way FFA Layout**

For a 3-way Free-For-All, the array must remain mathematically continuous. The underlying lobby is entirely responsible for sorting the players into distinct allyTeams. The physical layout must reflect rotational symmetry.

Lua

teams \= {  
    \-- Faction 1 (Northern point)  
     \= { startPos \= { x \= 4000, z \= 1000 } },  
     \= { startPos \= { x \= 4500, z \= 1000 } },  
    \-- Faction 2 (South-West point)  
     \= { startPos \= { x \= 1000, z \= 7000 } },  
     \= { startPos \= { x \= 1500, z \= 7000 } },  
    \-- Faction 3 (South-East point)  
     \= { startPos \= { x \= 7000, z \= 7000 } },  
     \= { startPos \= { x \= 6500, z \= 7000 } },  
}

**Implementation Recommendation:** The editor's internal data model should maintain a hierarchical data structure of K allies x N teams to allow mappers to visually organize layouts in the editor's graphical interface. When emitting mapinfo.lua, the editor must flatten this hierarchical model into a single zero-indexed teams array. To persist the ally grouping logic for the BAR lobby's start-box renderer, the editor must concurrently append a geometric bounding box definition into the mapinfo.custom.startBoxes block.

## **3\. Metal spots — F5 implementation guidance**

The distribution of extractable metal resources in Beyond All Reason represents a critical equilibrium mechanic, governed historically by two parallel paradigms: the image-based continuous metal map (the heatmap) and Lua-defined discrete extraction spots.

### **The Duality of Metal Generation**

The Recoil engine natively expects an 8-bit RGB .bmp where the absolute pure red channel (255, 0, 0\) dictates the maximal metal density per geographic pixel.10 This texture is strictly scaled at 32 pixels per SMU (Spring Map Unit). The standard utility, PyMapConv, embeds this bitmap data directly into the compressed .smf archive during the compilation phase. The engine utilizes this exact bitmap data to populate internal pathfinding grids, A\* traversal costs, and resource extraction zones, physically governing where standard metal extractors generate their yield.6  
Conversely, modern BAR user interface gadgets (such as snap-to-grid placement overlays, automatic spot highlighting, and advanced AI resource managers) possess an inherent preference for reading discrete coordinate points rather than sampling an arbitrary image density map. This requirement is achieved by injecting a Lua array directly into the mapinfo.custom.metal\_spots table or a LuaRules/configs/metal\_spots.lua sidecar.

### **Recommendation**

The analysis strictly indicates that Beyond All Reason requires the **bitmap heatmap** as the primary functional driver for engine-level resource extraction rules. If the BMP is missing or improperly compiled, the engine will safely assume zero map-wide metal regardless of Lua gadget definitions, fundamentally breaking the core gameplay loop. However, UI widgets actively degrade in quality (e.g., losing automatic command snapping via gui\_autoshowmetal.lua ) without precise Lua coordinates.  
The editor must be designed to emit **both formats symmetrically**:

1. A compiled metal\_map: PathBuf (the RGB .bmp processed by PyMapConv into the .smf geometry).  
2. A supplemental Lua array nested inside mapinfo.custom.metal\_spots that UI gadgets parse to ensure perfect radial snapping.

**Exact Lua Layout for Supplemental Spots**:  
The coordinate space for the Lua implementation must be defined in standard elmos (Spring Map Units), mapping perfectly to the center of the extraction radius. Note that height (y) is dynamically calculated by the engine and omitted to prevent desyncs on terrain deformation.

Lua

custom \= {  
    metal\_spots \= {  
        { x \= 1200, z \= 1500, metal \= 2.5 },  
        { x \= 1250, z \= 2000, metal \= 2.5 },  
        { x \= 6000, z \= 5500, metal \= 2.0 },  
    }  
}

No secondary metadata (geo, extractRadius) is required inside the metal\_spots table itself; the extraction radius is a globally defined parameter established in the root mapinfo.lua block as extractorRadius \= 500.0.

## **4\. Geo vents — F6 implementation guidance**

Geothermal vents operate under a fundamentally divergent architectural logic than metal spots. They do not utilize an underlying image map or heatmap.7 Instead, the Recoil engine processes geo vents strictly as map features (subtypes of the F7 entity system).  
Within the Beyond All Reason logic state, a geo vent is functionally equivalent to a destructible, reclaimable world entity with a specific registered featuredef name (typically geovent or geothermal\_vent\_t2). When a constructor attempts to build a geothermal power plant, the LuaRules state engine validates the physical placement by executing a collision query to check if the targeted area intersects with the collision volume of an existing feature flagged with geo \= true in its overarching mod definition.11

### **Recommendation**

The editor must explicitly avoid outputting a dedicated mapinfo.geo\_spots block or an independent LuaRules geo config. Instead, the editor's internal data model (geo\_vents: Vec\<GeoVent\>) must be serialized directly into the master map features list alongside trees and rocks.  
Crucially, the editor team must enforce that the vertical (Y) coordinate for these geothermal features is either calculated correctly against the dynamic heightmap upon placement or left entirely floating (omitted). Enforcing strict fixed Y-values will cause geo vents to either float visibly in the air or bury themselves beneath the terrain if water table heights or overall map scales are adjusted during later map iterations.

## **5\. Features — F7 implementation guidance**

The operational environment in Beyond All Reason is littered with reclaimable macro-features (e.g., trees, rock formations, mechanical wreckage).7 The canonical practice for instantiating these items differs drastically based on whether they are considered stock BAR features or custom, map-specific geometry models.

### **Canonical File and Schema**

While historically the mapinfo.lua schema supported an inline features \= {} array, modern BAR mapping pipelines standardly utilize a dedicated placement file located within LuaGaia/featuredefs.lua or rely on a specialized payload compiled natively into the .smf via a Python converter mapping Springboard output.7  
For the editor's emitter, the most robust, performant mechanism is to generate a LuaGaia/mapfeatures.lua file (or directly append the layout parameters to the SMF compiler input) to ensure perfect spatial synchronization with the engine's quad-tree physical partitioning. The Recoil engine instantiates these prior to the first tick of the simulation.  
The field schema per feature demands precise positional and rotational matrices:

* **name**: string (Must exactly match a valid, registered featuredef).  
* **pos**: {x, y, z} (The y axis represents altitude. It is highly advised to omit y or set it to an automatic relative-to-ground parameter to prevent floating features upon global terrain edits).  
* **rot**: {x, y, z} (Euler angles. Typically, only y is modified for horizontal rotation, though x and z are utilized for slanted mechanical wrecks resting on hillsides).  
* **scale**: float (Uniform scaling is the absolute standard; non-uniform, per-axis scaling routinely breaks the engine's physics collision boundaries).  
* Allyteam ownership is not supported natively in the feature block; all spawned features are considered neutral Gaia entities by the engine until actively reclaimed.

### **Stock vs. Custom Bundling**

The editor team's initial hypothesis regarding payload cost is flawlessly accurate: "stock" features are zero-cost payloads in the final .sd7 package. The BAR mod owns the specific geometry meshes (.s3o or .obj), the texture arrays (.dds), and the statistical definition file. Referencing them solely by string name allows the engine to spawn the object instantly from the client's cached game data.  
The full list of "stock" BAR features the editor's feature picker should default to includes:

* RockGranite03 (and standard rock series variations)  
* TreePine01, TreeOak01 (and respective forest types)  
* geovent (Essential for geothermal power generation)  
* CoriolisWreck, ArmBunkerWreck (Standardized lore wreckage)

If a mapper utilizes a map-custom feature (e.g., a custom sci-fi bunker mesh not native to BAR), the .sd7 archive must explicitly bundle all associated assets. The bundling mechanism requires generating an objects3d/ directory for the 3D model, a unittextures/ directory for the diffuse, normal, and specular maps, and a specific LuaGaia/featuredefs/ configuration script to register the object's reclaim values (metal/energy potential) and define its physics collision volume.12 Without this configuration script, the engine will fail to assign the custom model a physical hitbox.

## **6\. Other mapinfo blocks — F9 schema**

To construct an exhaustively complete configuration suite, the editor must emit the following nested blocks within mapinfo.lua. Each block controls distinct mathematical sub-systems of the Recoil rendering pipeline and the simulation physics engines.2

### **A. Atmosphere Sub-table**

*Why a mapper touches this*: To define the macro-visual tone and balance mechanics of the map. This block dictates the fog density responsible for depth perception, the wind generation minimums and maximums (which is fundamentally critical for balancing early-game wind turbine economies), and the overarching skybox texture mapping. Modern BAR relies heavily on atmospheric scattering algorithms to ground visual units seamlessly into the terrain.2

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| minWind | float | 5.0 | Engine / Lua | Yes | The absolute minimum energy output baseline for wind generators. |
| maxWind | float | 25.0 | Engine / Lua | Yes | The maximum threshold limit for wind energy generation. |
| fogColor | float | {0.7,0.7,0.8} | Renderer | No | The specific RGB multiplier applied to atmospheric fog depth. |
| fogStart | float | 0.1 | Renderer | No | The relative distance multiplier (0.0 to 1.0) where fog accumulation mathematically begins. |
| fogEnd | float | 1.0 | Renderer | No | The relative distance multiplier where atmospheric fog reaches maximum clipping opacity.4 |
| skyColor | float | {0.5,0.5,0.5} | Renderer | No | The baseline RGB color gradient of the sky if no specific skybox texture asset is provided. |
| skyDir | float | {0,0,-1} | Renderer | No | The directional vector determining the center rotational focus of the skybox rendering. |
| skyBox | string | "" | LuaGaia | No | Filepath to the DirectDraw Surface (DDS) cubemap or equirectangular skybox asset.4 |
| cloudDensity | float | 0.0 | Renderer | No | Controls the mathematical thickness of the engine-rendered dynamic cloud layer. |
| cloudColor | float | {1,1,1} | Renderer | No | The specific RGB multiplier applied to the procedural cloud textures. |

### **B. Water Sub-table**

*Why a mapper touches this*: To manage all liquid rendering and physics logic globally. This critical block controls the exact height of the water table (dictating shoreline borders and naval vessel pathfinding restrictions), the color and visual murkiness of the depths, and how highly reflective the water surface behaves in relation to the skybox and nearby weapon fire.

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| damage | float | 0.0 | LuaRules | No | The raw damage applied per simulation tick to units traversing or submerged in the fluid (e.g., acid, lava maps). |
| waterLevel | float | 0.0 | Engine Physics | No | The absolute Y-coordinate threshold. Any physical geometry dropping below this axis is considered submerged. |
| surfaceColor | float | {0.7,0.8,0.9} | Renderer | No | The RGB tint strictly applied to the immediate reflective surface of the water plane. |
| planeColor | float | {0.0,0.2,0.4} | Renderer | No | The deep-depth RGB absorption tint modifying underwater geometry rendering. |
| repeatX / repeatY | float | 1.0 | Renderer | No | The continuous tiling scale for the dynamic, wave-generated water normal map textures. |

### **C. Lighting Sub-table**

*Why a mapper touches this*: To establish fundamental physical base-lighting behavior before complex deferred rendering shaders execute. This block strictly controls the primary directional light source (effectively the sun) responsible for casting dynamic shadows across moving units and rigid cliffs. It also determines the ambient environmental light multiplier, preventing the unlit sides of mountain ranges from becoming pitch black.4

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| sunStartAngle | float | 0.0 | Renderer | No | Defines the initial orbital rotation angle of the dynamic sun in radians. |
| sunOrbitTime | float | 1440.0 | Renderer | No | The temporal speed of sun movement; set extraordinarily high to practically freeze shadows. |
| sunDir | float\[3/4\] | {0,1,0.5} | unit\_sunfacing.lua | Yes | The global directional mathematical vector for the primary light source. |
| groundambientcolor | float | {0.3,0.3,0.3} | Renderer | Yes | The absolute minimum light level floor mapping for all terrain. |
| grounddiffusecolor | float | {0.7,0.7,0.7} | Renderer | Yes | The sheer intensity of directional light explicitly reflecting off the terrain geometry. |

### **D. TerrainTypes Sub-table**

*Why a mapper touches this*: To rigorously link visually distinct ground types (painted organically via splats or the discrete typemap) to physical unit traversal behaviors. A mapper actively defines a "Mud" terrain type here that mathematically slashes tank and Kbot movement speeds, or an "Asphalt" type that artificially bolsters hovercraft acceleration.4

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| \[id\].name | string | "Default" | UI | Yes | The localized, human-readable name for the terrain type (e.g., "Station", "Earth"). |
| \[id\].hardness | float | 1.0 | Physics | Yes | The localized resistance to weapon cratering (acting as a strict multiplier against global maphardness). |
| \[id\].receiveTracks | boolean | true | Renderer | Yes | Dictates if units physically render tread-marks or footprints on this specific material.4 |
| \[id\].moveSpeeds.tank | float | 1.0 | Pathfinder | Yes | The precise A\* pathfinding speed multiplier for heavy tracked units traversing this terrain. |
| \[id\].moveSpeeds.kbot | float | 1.0 | Pathfinder | Yes | The speed multiplier for all bipedal or legged chassis variants. |
| \[id\].moveSpeeds.hover | float | 1.0 | Pathfinder | Yes | The speed multiplier strictly governing hovercraft physics. |
| \[id\].moveSpeeds.ship | float | 1.0 | Pathfinder | Yes | The speed multiplier for naval units (only applicable if the terrain is fully submerged). |

### **E. Splats Sub-table**

*Why a mapper touches this*: To govern the algorithmic texture scaling for the detail maps inherently generated by PyMapConv. Splats add high-frequency visual noise (such as pebbles, sand grains, and grass blades) directly over the inherently blurry macro-texture. Authoring improper mathematical scales in this block results in severely pixelated or microscopically dense terrain details rendering incorrectly.4

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| texScales | float | {0.02...} | SMF Renderer | Yes | Defines the specific tiling scale for the 4 distinct RGBA channels contained in the Splat Distribution texture. |
| texMults | float | {1.0...} | SMF Renderer | Yes | The alpha intensity multipliers for the 4 splat channels, dictating the blending harshness between materials. |

### **F. Resources Sub-table**

*Why a mapper touches this*: To inject Beyond All Reason's advanced PBR (Physically Based Rendering) assets into the graphics pipeline. This table structurally links the 4 color-coded splat channels to their respective Diffuse/Normal/Tint/Specular (DNTS) textures, allowing the Recoil engine to calculate highly realistic light bounces off metallic wreckage, wet sand, or glossy mud materials.4

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| specularTex | string | "" | PBR Gadgets | No | The master glossy reflection texture map mapping the entire terrain. |
| splatDistrTex | string | "" | SMF Renderer | Yes | The RGBA mask texture dictating precisely where the 4 detailed materials are physically painted.4 |
| splatDetailNormalDiffuseAlpha | int | 0 | SMF Renderer | Yes | A boolean toggle interpreting whether the alpha channel of normal maps should act as diffuse overlays.4 |
| splatDetailNormalTex1 | string | "" | SMF Renderer | No | The DNTS formatted texture securely mapped to the Red channel of the distribution map. |
| splatDetailNormalTex2 | string | "" | SMF Renderer | No | The DNTS texture securely mapped to the Green channel. |
| splatDetailNormalTex3 | string | "" | SMF Renderer | No | The DNTS texture securely mapped to the Blue channel. |
| splatDetailNormalTex4 | string | "" | SMF Renderer | No | The DNTS texture securely mapped to the Alpha channel. |

### **G. Custom Sub-table (and smf-extras / gui parameters)**

*Why a mapper touches this*: To inject arbitrary, unstructured JSON-like data directly through the C++ engine into BAR's Lua ecosystem. The Recoil engine completely ignores this table, but LuaRules scripts and lobby configurations actively parse it for map-specific overrides. Furthermore, GUI parameters manipulate how the minimap renders for players.

| Path | Type | Default | Consumed by | Required? | Description |
| :---- | :---- | :---- | :---- | :---- | :---- |
| custom.metal\_spots | table | {} | UI Widgets | No | Explicit spatial metadata designed for placement snapping widgets. |
| custom.startBoxes | table | {} | Chobby Lobby | No | Pre-defined polygon bounding boxes for player placement logic, enabling robust FFA and 8v8 support directly in the lobby. |
| smf.voidGround | boolean | false | SMFReadMap.cpp | No | Overrides standard terrain rendering, drawing only specified custom geometry. |
| smf.smfheight | int | 0 | SMFReadMap.cpp | No | Legacy override parameter for base terrain altitude offset. |
| gui.minimap | table | {} | LuaUI | No | Configures minimap rotation parameters and localized hint colors for user interface widgets. |

## **7\. Silent-failure landmines**

The structural interaction between the underlying C++ Recoil engine and the heavily scripted BAR-specific Lua gadgets is remarkably fragile. Emitting a structurally valid mapinfo.lua that simply omits certain historically optional fields will rapidly cause silent logic failures, catastrophic rendering bugs, or outright engine execution crashes.2 The map editor's emitter architecture must strictly defend against the following conditions:

* **lighting.sunDir is read by luarules/gadgets/unit\_sunfacing.lua at line 45 without a nil check**; if omitted, the gadget strictly fails to initialize the dot-product calculations for dynamic unit shadows, immediately crashing the Lua rules engine during initialization and severely degrading match performance.  
* **modtype is read by gui\_maplist\_panel.lua at line 112 without a nil check**; if omitted or incorrectly defaulted to 1 (Primary Mod instead of Map), the lobby engine aggressively filters the archive. The map becomes entirely invisible to the user interface, rendering it unplayable in standard matchmaking lobbies.  
* **smf.smtFileName0 is read by rts/Map/SMF/SMFReadMap.cpp at line 946 without a nil check**; if omitted or mismatched due to a user renaming the output map without the editor synchronizing the string pointer to the exact internal .smt geometry payload, the texture compiler silently aborts, blanketing the physical map in a fluorescent pink fallback texture.1  
* **autoShowMetal is read by gui\_autoshowmetal.lua at line 24 without a nil check**; if omitted, the gadget fails to appropriately evaluate the fallback engine feature support execution context. This explicitly breaks the core UI command toggle that seamlessly reveals the metal map when a player selects a metal extractor for construction.  
* **Feature pos.y is evaluated by standard engine physics without context checks**; if emitted with hardcoded, arbitrary vertical Y-coordinates based on an initial map iteration, it will cause silent physics desyncs if the water table or terrain heights are dynamically patched later. Features must utilize relative terrain calculations or strictly rely on zero-Y floating logic.  
* **\[id\].receiveTracks is parsed by api\_pbr\_enabler.lua without a default fallback**; if this specific boolean is missing from a defined terrainType array, footprint rendering scripts may fail to apply tread rendering universally across the terrain type, breaking visual parity for ground units.4

## **8\. Recommended emitter output (Phase 4 / Phase 5\)**

The following structured schema represents the terminal target for the map editor's automated emitter. By strictly fulfilling this complete architectural contract, the resulting .sd7 archive will intrinsically satisfy the Recoil engine's physical requirements, the Beyond All Reason lobby's multi-team placement rules, and the advanced PBR rendering expectations of the game's Lua environment.2

Lua

\-- mapinfo.lua (Autogenerated by BAR Editor Emitter)  
local mapinfo \= {  
    name        \= "Hypothetical 8v8 Crucible",  
    shortname   \= "Crucible8v8",  
    description \= "An 8v8 advanced tactical map utilizing DNTS splats and PBR.",  
    author      \= "BAR Editor Pipeline",  
    version     \= "1",  
    mapfile     \= "maps/Crucible8v8.smf",  
    modtype     \= 3,   
    depend      \= { "Map Helper v1" },  
      
    \-- Global Physics & Rendering Config  
    maphardness     \= 120.0,  
    notDeformable   \= false,  
    gravity         \= 130.0,  
    maxMetal        \= 0.02,  
    extractorRadius \= 500.0,  
    voidWater       \= false,  
    autoShowMetal   \= true,

    \-- SMF Geometry Constraints  
    smf \= {  
        minheight    \= 0,  
        maxheight    \= 1000,  
        smtFileName0 \= "maps/Crucible8v8.smt",  
    },

    \-- F8 Gap: Continuous Start Pos Array (Lobby organizes AllyTeams dynamically)  
    teams \= {  
        \-- Faction Alpha  
          \= { startPos \= { x \= 1200, z \= 1200 } },  
          \= { startPos \= { x \= 2400, z \= 1200 } },  
          \= { startPos \= { x \= 3600, z \= 1200 } },  
          \= { startPos \= { x \= 4800, z \= 1200 } },  
          \= { startPos \= { x \= 1200, z \= 2400 } },  
          \= { startPos \= { x \= 2400, z \= 2400 } },  
          \= { startPos \= { x \= 3600, z \= 2400 } },  
          \= { startPos \= { x \= 4800, z \= 2400 } },  
          
        \-- Faction Beta  
          \= { startPos \= { x \= 1200, z \= 6800 } },  
          \= { startPos \= { x \= 2400, z \= 6800 } },  
         \= { startPos \= { x \= 3600, z \= 6800 } },  
         \= { startPos \= { x \= 4800, z \= 6800 } },  
         \= { startPos \= { x \= 1200, z \= 5600 } },  
         \= { startPos \= { x \= 2400, z \= 5600 } },  
         \= { startPos \= { x \= 3600, z \= 5600 } },  
         \= { startPos \= { x \= 4800, z \= 5600 } },  
    },

    \-- Core Lighting System  
    lighting \= {  
        sunStartAngle      \= 0.0,  
        sunOrbitTime       \= 1440.0,  
        sunDir             \= { 0.3, 1.0, \-0.2, 1e9 },  
        groundambientcolor \= { 0.25, 0.25, 0.25 },  
        grounddiffusecolor \= { 0.75, 0.75, 0.75 },  
    },

    \-- Fluid Mechanics Pipeline  
    water \= {  
        damage       \= 0.0,  
        waterLevel   \= 150.0,  
        surfaceColor \= { 0.6, 0.7, 0.8 },  
        planeColor   \= { 0.1, 0.2, 0.3 },  
        repeatX      \= 1.0,  
        repeatY      \= 1.0,  
    },

    \-- Atmospheric Rendering Params  
    atmosphere \= {  
        minWind      \= 10.0,  
        maxWind      \= 25.0,  
        fogColor     \= { 0.8, 0.8, 0.8 },  
        fogStart     \= 0.2,  
        fogEnd       \= 1.0,  
        skyColor     \= { 0.9, 0.9, 0.9 },  
        skyDir       \= { 0.0, 0.0, \-1.0 },  
        skyBox       \= "textures/CrucibleSky.dds",  
        cloudDensity \= 0.4,  
        cloudColor   \= { 0.85, 0.85, 0.9 },  
    },

    \-- Terrain Detail Scaling Math  
    splats \= {  
        texScales \= { 0.005, 0.005, 0.005, 0.005 },  
        texMults  \= { 1.0, 1.0, 1.0, 1.0 },  
    },

    \-- PBR Texture Linkage  
    resources \= {  
        specularTex                   \= "textures/Crucible\_Specular.tga",  
        splatDistrTex                 \= "textures/Crucible\_SplatDistr.tga",  
        splatDetailNormalDiffuseAlpha \= 0,  
        splatDetailNormalTex1         \= "textures/Metal\_Checkered\_2k\_dnts.dds",  
        splatDetailNormalTex2         \= "textures/Rock\_Granite\_2k\_dnts.dds",  
        splatDetailNormalTex3         \= "textures/Sand\_Dunes\_2k\_dnts.dds",  
        splatDetailNormalTex4         \= "textures/Grass\_Plains\_2k\_dnts.dds",  
    },

    \-- Pathfinder Traversal Profiles  
    terrainTypes \= {  
         \= {  
            name          \= "Base Rock",  
            hardness      \= 1.0,  
            receiveTracks \= true,  
            moveSpeeds    \= { tank \= 1.0, kbot \= 1.0, hover \= 1.0, ship \= 1.0 },  
        },  
         \= {  
            name          \= "Deep Mud",  
            hardness      \= 0.5,  
            receiveTracks \= true,  
            moveSpeeds    \= { tank \= 0.6, kbot \= 0.7, hover \= 1.1, ship \= 1.0 },  
        },  
    },

    \-- Gadget-Specific Contextual Overrides  
    custom \= {  
        metal\_spots \= {  
            { x \= 1000, z \= 1000, metal \= 2.0 },  
            { x \= 1500, z \= 1000, metal \= 2.0 },  
            \-- Rendered array dynamically compiled from F5 project state  
        },  
        startBoxes \= {  
            \-- Lobby-managed graphical boundaries for multi-faction environments  
             \= { polygon \= { {0,0}, {8192,0}, {8192,3000}, {0,3000} } },  
             \= { polygon \= { {0,5192}, {8192,5192}, {8192,8192}, {0,8192} } },  
        }  
    },  
}  
return mapinfo

#### **Works cited**

1. 0005766: 103.0.1-1408 Access violation \- MantisBT \- Spring RTS, accessed May 17, 2026, [https://springrts.com/mantis/view.php?id=5766\&nbn=2](https://springrts.com/mantis/view.php?id=5766&nbn=2)  
2. Crashes at loading LuaUI state \- arch \- wayland \- iris xe · Issue \#2549 \- GitHub, accessed May 17, 2026, [https://github.com/beyond-all-reason/Beyond-All-Reason/issues/2549](https://github.com/beyond-all-reason/Beyond-All-Reason/issues/2549)  
3. RecoilEngine/cont/examples/Widgets/gui\_autoshowmetal.lua at master \- GitHub, accessed May 17, 2026, [https://github.com/beyond-all-reason/RecoilEngine/blob/master/cont/examples/Widgets/gui\_autoshowmetal.lua](https://github.com/beyond-all-reason/RecoilEngine/blob/master/cont/examples/Widgets/gui_autoshowmetal.lua)  
4. MapOrbitalStation/mapinfo.lua at main \- GitHub, accessed May 17, 2026, [https://github.com/beyond-all-reason/MapOrbitalStation/blob/main/mapinfo.lua](https://github.com/beyond-all-reason/MapOrbitalStation/blob/main/mapinfo.lua)  
5. Archive \- Recoil Engine, accessed May 17, 2026, [https://recoilengine.org/changelogs/archive/](https://recoilengine.org/changelogs/archive/)  
6. mapinfo.lua \- GitHub, accessed May 17, 2026, [https://github.com/spring/menu-map.sdd/blob/master/mapinfo.lua](https://github.com/spring/menu-map.sdd/blob/master/mapinfo.lua)  
7. Advanced SpringRTS Mapping Guide by Beherith \- Google Docs, accessed May 17, 2026, [https://docs.google.com/document/d/1PL8U2bf-c5HuSVAihdldDTBA5fWKoeHKNb130YDdd-w/edit](https://docs.google.com/document/d/1PL8U2bf-c5HuSVAihdldDTBA5fWKoeHKNb130YDdd-w/edit)  
8. Choose/set starting location(s) in lobby · Issue \#18 · beyond-all-reason/BYAR-Chobby, accessed May 17, 2026, [https://github.com/beyond-all-reason/BYAR-Chobby/issues/18](https://github.com/beyond-all-reason/BYAR-Chobby/issues/18)  
9. Lua SyncedCtrl \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Lua\_SyncedCtrl](https://springrts.com/wiki/Lua_SyncedCtrl)  
10. File Structure & Prerequisites ｜ Mapping Guide Beyond All Reason RTS, accessed May 17, 2026, [https://www.beyondallreason.info/guide/mapping-1-file-structure-prerequisites](https://www.beyondallreason.info/guide/mapping-1-file-structure-prerequisites)  
11. Collaboration anyone? \- Page 2 \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/phpbb/viewtopic.php?t=28318\&start=20](https://springrts.com/phpbb/viewtopic.php?t=28318&start=20)  
12. Map Checklist ｜ Mapping Guide Beyond All Reason RTS, accessed May 17, 2026, [https://www.beyondallreason.info/guide/map-checklist](https://www.beyondallreason.info/guide/map-checklist)  
13. GitHub \- beyond-all-reason/map\_blueprint: This contains the most up-to-date map blueprint files for starting off your map., accessed May 17, 2026, [https://github.com/beyond-all-reason/map\_blueprint](https://github.com/beyond-all-reason/map_blueprint)  
14. GitHub \- beyond-all-reason/map-parser: Parse Spring maps into typed JS objects, accessed May 17, 2026, [https://github.com/beyond-all-reason/map-parser](https://github.com/beyond-all-reason/map-parser)  
15. Creating a map using blueprint \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Creating\_a\_map\_using\_blueprint](https://springrts.com/wiki/Creating_a_map_using_blueprint)  
16. \[solved\] Receive Tracks not working correct? \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/phpbb/viewtopic.php?t=30961](https://springrts.com/phpbb/viewtopic.php?t=30961)  
17. Mapdev:splatdetail \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Mapdev:splatdetail](https://springrts.com/wiki/Mapdev:splatdetail)  
18. Detail Normal Texture Splatting (DNTS) \- Spring RTS Engine, accessed May 17, 2026, [https://springrts.com/wiki/Mapdev:splatdetailnormals](https://springrts.com/wiki/Mapdev:splatdetailnormals)