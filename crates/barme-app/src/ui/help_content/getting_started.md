# Getting started

The BAR Map Editor is a standalone Rust + egui + wgpu desktop GUI
for authoring Beyond All Reason / Recoil maps. From an empty
project you can sculpt terrain, paint texture layers, place metal
spots, geo vents, and features, configure start positions, and
build a playable `.sd7` archive that BAR installs into its user
maps directory.

The editor is structured around nine tools listed down the left
strip: `Q` Select / orbit, `B` Sculpt, `S` Start positions, `M`
Metal spots, `V` Geo vents, `F` Features, `W` Water / Lava, `L`
Paint layer, `G` Procgen. The Inspector on the right tracks the
active tool and surfaces just the controls that apply. The
central viewport is a 3D orbit view (top-down 2D when the Paint
layer tool is active).

The build pipeline shells out to PyMapConv to compile SMF + SMT,
then packages a non-solid 7z `.sd7`. The output drops into BAR's
maps directory; the next BAR launch picks it up automatically and
it shows in the Skirmish lobby. See **Build pipeline** for the
full chain and **Pitfalls** for the silent-failure rules the
editor enforces.

## First map in ten minutes

1. Pick a biome and SMU size in the wizard. The wizard seeds a
   demo heightmap so you have something to look at immediately.
2. Press **B** to enter Sculpt, then drag in the 3D viewport to
   raise terrain. **Ctrl-Z** undoes.
3. Press **M**, click to drop a couple of metal spots. **S** lets
   you place start positions per ally team.
4. Click **Build + Install** in the top bar. Watch the log panel
   for PyMapConv output. The output `.sd7` lands at
   `~/.local/state/Beyond All Reason/maps/`.
5. Launch BAR. Your map appears in the Skirmish list (unofficial
   maps don't surface in multiplayer lobbies — see PITFALL §B).
