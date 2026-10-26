# Operation Iron Ridge

An open-world military sandbox in the spirit of Arma, built with [Bevy](https://bevy.org) 0.20.
You fight across a 4 × 4 km island with infantry, vehicles and a helicopter, either alone with an AI
fireteam or in co-op with friends.


## Features

- **Open world.** A procedurally generated 4 km island with hills, forests (about 32,000 trees),
  roads, eight towns and outposts, and a coastal forward operating base. A topographic map (`M`)
  shows contours, roads, buildings, the 100 m / 1 km grid, units and sector ownership.
- **Dynamic campaign.** Every town is a sector held by an OPFOR garrison of defenders, patrols and
  vehicles. Garrisons are only simulated while players are within about 1 km. When everyone leaves,
  they are cached along with their losses. Clear and hold a sector to capture it, then expect a
  motorized counterattack. An allied AI squad pushes toward the nearest enemy sector on its own.
  Hold every sector to win.
- **Infantry combat.**
  - Ballistics with travel time, gravity, drag and adjustable zeroing.
  - Stances (stand, crouch, prone), sprint and stamina, aiming down the sights with red-dot optics,
    and free-look.
  - Recoil, fire modes, grenades, first aid, headshots and suppression.
  - Supersonic cracks as rounds pass close by.
  - Roles: team leader, rifleman, autorifleman, AT specialist.
- **Vehicles.**
  - HMMWV with an M2 .50 cal.
  - Bradley/BMP IFV with a 30 mm cannon and coax MG.
  - UH-1Y helicopter with minigun and rockets, flown with collective and cyclic.

  Vehicles follow the terrain, flatten trees, run people over, burn when destroyed and respawn at
  the FOB. Their turrets follow your aim, with automatic drop compensation.
- **AI.**
  - **Squads** take orders (form on me, hold, move, patrol, attack, defend) and keep a wedge formation.
  - **Spotting** depends on line of sight, field of view, stance, movement, muzzle flash and
    foliage. Soldiers react with a delay and aim with an error that tightens over time.
  - **Engagement:** they lead moving targets, compensate for drop and hold fire when friendlies are
    in the way. AT soldiers switch to launchers against vehicles.
  - **Squad behaviour:** contacts are shared within the squad, and soldiers react to gunfire and
    suppression.
  - **Vehicle AI** drives, fights and dismounts troops.
- **Co-op multiplayer.**
  - Host-authoritative networking over UDP, built on a custom binary protocol.
  - Each player gets their own AI fireteam. If a player disconnects, the AI takes over their soldier.
- **Procedural audio.** Every sound is synthesized at startup: gunshots with distant, echoing
  variants, explosions, rockets, cracks, impacts, reloads, and engine and rotor loops. Sounds are
  panned in stereo and delayed by the speed of sound.
- **No asset files.** All models, textures, the map and the sounds are generated in code.


## Building and running

You need a recent stable Rust toolchain (the project uses edition 2024; Bevy 0.20 requires Rust 1.97 or newer).
On Linux you also need Bevy's system dependencies (ALSA, udev, and Wayland/X11 development packages).
See [Bevy's Linux dependency guide](https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md).

| Command | What it does |
| --- | --- |
| `cargo dev` | Debug build with **Bevy dynamically linked** (fast incremental rebuilds), then run |
| `cargo dev-build` | Same, build only |
| `cargo run --release` | Optimized build with **Bevy statically linked** |
| `cargo build --release` | Build the release binary at `target/release/milsim` |

The `dev` aliases live in `.cargo/config.toml`. They enable the `dev` feature, which turns on
`bevy/dynamic_linking`. Release builds never enable it, so `target/release/milsim` is a
self-contained executable. A dynamically linked debug binary needs Bevy's shared library on the
library path, so start it through `cargo dev` rather than running `target/debug/milsim` directly.

Dependencies are compiled with full optimizations even in debug builds, so the game is playable
while you iterate. The first build takes a while.

## Graphics quality and weak computers

The game picks a graphics preset from your GPU at startup. Integrated and software renderers get
**Low**; dedicated GPUs get **High**. Press **F9** in game to cycle presets, or choose one on the
command line:

```sh
cargo run --release -- --quality low          # low | medium | high
cargo run --release -- --render-scale 0.5     # render the 3D scene at 50% resolution
```

| | Low | Medium | High |
| --- | --- | --- | --- |
| 3D render resolution | 60% | 80% | 100% |
| Anti-aliasing | none | FXAA | 4× MSAA |
| Shadows | off | one cascade, 70 m | three cascades, 220 m |
| View distance | 900 m | 1.4 km | 1.9 km |
| Tree draw distance | 500 m | 900 m | 1.5 km |

The HUD always renders at full resolution. On an Intel UHD 620 laptop (i5-10210U) the game runs
at about 60 fps on Low (the display's refresh cap), 38 fps on Medium and 16 fps on High. On very weak
GPUs, lower `--render-scale` further (down to 0.25).

## Multiplayer

```sh
# Host (also plays). Default UDP port 27015.
cargo run --release -- host
cargo run --release -- host 28000

# Join
cargo run --release -- join 192.168.1.20
cargo run --release -- join 192.168.1.20:28000 MyName
```

Forward or allow the UDP port on the host for internet play. The host simulates everything. Clients
send their input and receive snapshots 20 times a second, and render interpolated state. Clients do
no prediction, so a low-latency connection gives the best feel.

## Controls

| Infantry | |
| --- | --- |
| `W A S D` | Move |
| `Shift` | Sprint |
| `C` / `Z` | Crouch / prone (toggle) |
| Mouse | Look and aim |
| `Left Alt` (hold) | Free-look |
| `Left mouse` | Fire |
| `Right mouse` (hold) | Aim down sights (launchers fire only while aiming) |
| `R` | Reload |
| `B` | Toggle fire mode (semi/auto) |
| `1` / `2` / `X` | Primary / launcher / next weapon |
| `G` | Throw grenade |
| `H` | First aid |
| `Page Up` / `Page Down` | Zeroing (100 to 800 m) |
| `F` | Get in / out of the nearest vehicle |
| `V` | First / third person |
| `M` | Map (also lists the controls) |
| `F9` | Cycle graphics quality |
| `F1` / `F2` / `F3` | Squad: form on me / hold / move to crosshair |
| `Enter` | Redeploy after death (`1` to `4` picks a role first) |
| `Esc` / click | Release / capture the mouse |

| Vehicles | |
| --- | --- |
| `W A S D` | Throttle and steering |
| `Space` | Brake |
| Mouse | Camera; the turret follows the crosshair |
| `Left mouse` / `X` | Fire / switch weapon |
| `Right mouse` | Zoom in the gunner view (`V` toggles gunner view) |

| Helicopter | |
| --- | --- |
| `Shift` / `Ctrl` | Collective up / down |
| `W` / `S` | Pitch forward / back |
| `A` / `D` | Yaw (pedals) |
| `Q` / `E` | Roll |

Your AI squadmates board your vehicle when you order them to form on you (`F1`), and they dismount
when you get out.


## Code layout

| Module | Responsibility |
| --- | --- |
| `main.rs` | Command line, plugins and per-frame phases |
| `terrain.rs` | Heightfield, locations, roads, terrain meshes, map image |
| `world.rs` | Props (forests, towns, FOB), obstacle collision grid, line of sight |
| `models.rs` | Mesh helpers (bevelled boxes, limbs, tapered cylinders, rocks) |
| `soldier.rs` | Infantry models, movement, weapon handling, death |
| `combat.rs` | Weapon stats, ballistics, hit detection, explosions, damage, effects |
| `vehicle.rs` | Vehicle models, driving/flight physics, seats, turrets, AI drivers |
| `ai.rs` | Squads, formations, perception, engagement |
| `player.rs` | Input, respawn, camera, first-person view model |
| `mission.rs` | Sectors, garrisons, capture, counterattacks, FOB vehicles |
| `net.rs` | UDP protocol, replication, interpolation |
| `hud.rs` | HUD and tactical map |
| `audio.rs` | Sound synthesis and playback |
| `quality.rs` | Graphics presets, render scaling, distance culling |
| `devtools.rs` | Scripted screenshot tour |

Every frame runs the phases `Input → Ai → Movement → Combat → Damage → Camera → Hud`. Simulation
systems only run where the game is authoritative (single player or host). Clients run input,
replication and presentation.

### Screenshot tour

`MILSIM_TOUR=<dir> cargo dev` plays a scripted sequence. It covers first and third person, the map,
aiming, driving, a town assault and helicopter flight. It saves screenshots to `<dir>`, logs the
frame rate and main-world CPU time for each segment, and exits. It runs without vsync so the
numbers reflect real performance. As a client, it only looks around.

## License

[GNU Affero General Public License v3.0](LICENSE).
