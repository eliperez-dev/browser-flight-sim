# Browser Flight Simulator

A physics-accurate flight simulator running entirely in the browser via WebAssembly. Built with Rust, Bevy 0.18, and Avian3D — no backend, no install, just open and fly.

---

## Overview

This project ports a research-grade aerodynamic flight model (Khan & Nahon 2015) into a real-time 3D simulator with procedurally generated infinite terrain, streamed at runtime. Everything — physics, terrain, rendering, navigation — runs at 60 Hz in a WASM bundle served as a static site.

**Tech stack:** Rust · Bevy 0.18 (ECS game engine) · Avian3D (rigid-body physics) · bevy_egui (debug UI) · Trunk (WASM bundler) · Perlin noise terrain

---

## Features

### Flight Physics

The aerodynamic model is based on the Khan & Nahon 2015 academic paper, implemented surface-by-surface:

- **Control surfaces** — Wings, ailerons, elevator, rudder, and flaps are each independent physics components. Each surface computes lift and drag in its local chord plane using a full lift curve with configurable stall angles, then rotates forces into world space.
- **Ground effect** — A Gaussian proximity model increases effective wing aspect ratio near the ground, realistically boosting lift and reducing induced drag on landing and takeoff.
- **Trapezoidal velocity prediction** — A midpoint-accurate integrator improves numerical stability at low framerates.
- **Fixed-pitch propeller** — Thrust scales with a realistic RPM→thrust curve. A mixture control adjusts fuel-air ratio; pulling it to idle cutoff kills the engine.
- **Engine state machine** — Off → Cranking (starter key) → Running. Engine responds to throttle and mixture.
- **Servo lag** — All control inputs filter through a tunable time constant, preventing instantaneous surface deflection.
- **Fuselage drag** — Body-axis-aligned form drag models realistic high-AoA and crosswind behavior.
- **Rotational damping** — Per-axis damping stabilizes pitch, roll, and yaw.
- **Landing gear** — Three spring-damper struts (nose + two mains) sampled against terrain height. Tyre friction applies strong sideways resistance and light rolling resistance, generating correct pitch moments about CoM on ground.

All flight model constants are live-editable at runtime from the debug panel — no recompile needed to tune.

### Procedural World

An infinite, seeded world generated from multi-octave Perlin noise:

- **Biome climate system** — Continentalness, temperature, and humidity fields combine to produce deserts, grasslands, taiga, forests, snowy peaks, and ocean. Biome colors are painted as vertex colors on the terrain mesh.
- **Lapse-rate temperature** — Temperature drops with altitude, producing natural snow lines on peaks.
- **Latitude banding** — Warm equator strips and cold polar bands shape the climate map.
- **Deterministic** — The same seed always produces the same world. Fully reproducible.

### Terrain Streaming

The world is divided into 500 m × 500 m chunks, loaded and unloaded around the camera in real time:

- **LOD system** — Four distance bands with 12→8→3→1 mesh subdivisions from near to far.
- **Rate-limited generation** — At most 3 chunks spawn per frame to prevent WASM main-thread stalls.
- **Async mesh baking** — Mesh construction runs on the `AsyncComputeTaskPool`; on WASM this is carefully throttled to the main thread.
- **Chunk boundary tracking** — The grid only re-scans when the camera crosses a chunk edge, keeping overhead near zero between moves.
- **Render distance** — 28-chunk radius (~6 km), despawning chunks outside the window.

### Airports & Navigation

Airports are placed on an infinite 5 km grid, each cell seeded deterministically:

- **Five airport classes** — Dirt strips, small GA, large commuter, regional, and hub airports, distributed by cell hash.
- **Terrain flattening** — Terrain under each runway is flattened in an oriented rectangle and ramps smoothly back to natural height.
- **Runway lights** — Strobes, beacons, and approach lights animate based on engine and time-of-day state.
- **Runway identifiers** — Deterministic ICAO-style identifiers generated from world position.
- **Waypoint labels** — 3D stalk markers at runway positions with projected screen labels showing ident, airport name, and distance. Labels fade at range (visible 3–120 km).

### Rendering

- **Dual-camera pipeline** — An inner `Camera3d` renders the 3D world to a 640×360 render texture. An outer `Camera2d` upscales it to the window with integer pixel scaling, giving a clean retro aesthetic.
- **Day-night cycle** — Orbiting sun directional light with a 2,500-star procedural starfield and a rendered sun disc.
- **Atmospheric fog** — Distance fog with configurable base color, extinction, and inscattering. Sun-glow tinting blends with the directional light.
- **Ocean** — A single low-poly water plane at sea level follows the camera horizontally with a tunable PBR material.
- **Aircraft exterior lights** — Nav lights (red/green/white), strobes, belly beacon (pulses with engine), and a toggled landing light spotlight.
- **Low-poly aircraft** — GLTF Cessna 172 with animated propeller (angular velocity matches engine RPM).
- **Physics interpolation** — `TransformInterpolation` smooths the aircraft visually between fixed 60 Hz physics steps.

### Camera

Two modes, toggled with **F**:

- **Orbit camera** (default) — Follows the aircraft at 15 m radius. Arrow keys adjust pitch/yaw of the orbit angle.
- **Free camera** — WASD/QE movement at 5 m/s normal, 300 m/s with Shift, 2,000 m/s with double-Shift. Arrow keys look.

### Multiplayer

Client-authoritative, relay-only — the server never simulates flight:

- **Workspace layout** — `protocol/` defines the shared wire types; `server/` is an Axum WebSocket relay; `client/` runs the same terrain/physics locally for every player from a shared seed.
- **World registry** — the server multiplexes any number of worlds (the always-on default plus player-created ones) over one HTTP/WS listener, routed by a `ServerId` in the path; idle non-default worlds are reaped.
- **State relay** — each client simulates its own aircraft and broadcasts position/rotation/control-surface state; remote players are rendered as visual-only ghosts (no physics) interpolated between updates.
- **Server directory** — a lobby/browser for discovering and joining player-created worlds.

### Debug & Tuning

A full live-tuning system for iteration without recompiling:

- **F3 debug panel** — egui sliders for every physics constant: aero surface configs, weight & balance (fuel, cargo, occupants shift CoM and inertia live), engine/propeller params, servo response, gravity, air density, terrain generation (seed, scales, LOD), sky, water, fog, lights, and map display.
- **F4 map overlay** — 256×256 rendered world map with biome/height/category view tabs, aircraft and camera markers, airport overlays with runway lines, a 10-minute breadcrumb trail, click-to-select airports with Direct-To course line, and scroll/drag pan+zoom.
- **HUD** — Top-left text overlay: speed (m/s and knots), altitude, engine state, mixture, throttle, flaps, RPM, FPS.
- **G gizmos** — 3D wireframe overlays for CoM, velocity, thrust, gravity, and per-surface aerodynamic force vectors.

---

## Controls

| Input | Action |
|-------|--------|
| ↑ / ↓ | Elevator (pitch) |
| ← / → | Ailerons (roll) |
| A / D | Rudder (yaw) |
| = / - | Throttle up/down |
| < / > | Flaps extend/retract |
| K / L | Mixture lean/rich |
| I | Engine starter |
| B | Brakes |
| N | Landing light toggle |
| F | Camera mode toggle |
| P | Pause physics |
| F3 | Debug panel |
| F4 | Map overlay |
| G | Gizmos toggle |

---

## Architecture

A Cargo workspace of three crates: `client/` (the WASM app, Bevy ECS throughout), `server/` (the Axum WebSocket relay), and `protocol/` (wire types shared by both).

| Module (`client/src/`) | Responsibility |
|--------|---------------|
| `physics/` | Aerodynamics, engine, landing gear, flight config |
| `terrain/` | Noise generation, chunk streaming, LOD, runway placement |
| `camera.rs` | Free/orbit camera, canvas upscaling on resize |
| `sky.rs` | Day-night cycle, sun, stars, ambient light |
| `water.rs` | Ocean plane with tunable PBR material |
| `lights.rs` | Nav, strobe, beacon, and landing lights |
| `fog.rs` | Atmospheric fog with sun-directional tinting |
| `plane.rs` | Aircraft entity, GLTF mesh, propeller animation |
| `map/` | Debug world map, airport overlays, breadcrumb trail |
| `waypoints.rs` | In-world 3D runway stalk labels |
| `player_labels.rs` | On-screen name tags for remote players |
| `airport_names.rs` | Deterministic airport ident/name generation |
| `pilot_handbook.rs` | In-game reference window (controls, systems, airports) |
| `network.rs` / `network_directory.rs` | WebSocket client, remote-player sync, server browser |
| `ui/` | Menu bar and per-system settings panels (plane, camera, world, multiplayer, graphics) |
| `debug_tools/` | HUD, F3 tuning panel, gizmos |

**Physics runs at a fixed 60 Hz** (`PhysicsSchedule`) decoupled from render framerate. Custom force systems (`apply_aero_forces`, `apply_landing_gear`) register before the broad-phase collision pass.

---

## Running Locally

Requires Rust and [Trunk](https://trunkrs.dev/):

```sh
trunk serve          # dev server with hot reload
trunk build --release  # optimized WASM bundle → dist/
```

The `dist/` directory is a self-contained static site — drop it on any static host.

---

## References

- Khan, W. & Nahon, M. (2015). *Real-time modeling of agile fixed-wing UAV aerodynamics.* — basis for the aerodynamic surface model
- Low-poly Cessna 172 model — CC-licensed GLTF asset (`assets/low-poly-airplane/`)
