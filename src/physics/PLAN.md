# Aircraft Physics Port Plan

Port the Unity `Aircraft-Physics` aerodynamics model into this Bevy simulator,
replacing the current arcade `SimplePlanePhysics` with a proper rigid-body +
blade-element aerodynamics model.

Source reference: `Aircraft-Physics/Assets/Aircraft Physics/Core/Scripts/`

---

## Phase 0 — Add Avian Physics

Add `avian3d` to `Cargo.toml`:

```toml
avian3d = { version = "0.3", features = ["default"] }
```

In `main.rs`, register the plugin:

```rust
use avian3d::prelude::*;
App::new()
    .add_plugins((DefaultPlugins, PhysicsPlugins::default()))
    ...
```

Avian is chosen over bevy_rapier because it is ECS-native: forces and torques
are just components (`ExternalForce`, `ExternalTorque`), inertia is a
`RigidBody` component, and there is no special physics context to thread through
systems. This maps cleanly to the Unity `Rigidbody.AddForce` / `AddTorque`
calls in `AircraftPhysics.cs`.

---

## Phase 1 — Core Data Types

### `BiVector3` → `src/physics/bi_vector3.rs`

Direct port of `BiVector3.cs`. A paired (force, torque) struct.

```rust
#[derive(Default, Clone, Copy)]
pub struct BiVector3 {
    pub force:  Vec3,
    pub torque: Vec3,
}
// impl Add, Mul<f32>
```

### `AeroSurfaceConfig` → `src/physics/aero_surface_config.rs`

Port of `AeroSurfaceConfig.cs`. Pure data, becomes a Bevy `Component` (or an
`Asset` if we want to share configs between surfaces).

Fields: `lift_slope`, `skin_friction`, `zero_lift_aoa`, `stall_angle_high`,
`stall_angle_low`, `chord`, `flap_fraction`, `span`, `aspect_ratio`.

Validation (currently in Unity's `OnValidate`) runs once at spawn or whenever
the config changes.

---

## Phase 2 — AeroSurface Component + System

### `src/physics/aero_surface.rs`

**Component:**
```rust
#[derive(Component)]
pub struct AeroSurface {
    pub config:           AeroSurfaceConfig,
    pub is_control_surface: bool,
    pub input_type:       ControlInputType,  // Pitch | Yaw | Roll | Flap
    pub input_multiplier: f32,
    flap_angle:           f32,               // set each frame by controller
}
```

**System `calculate_aero_forces`** (runs in `PhysicsSchedule`):

For each `AeroSurface` child entity, compute lift + drag + torque using the
exact coefficient math from `AeroSurface.cs`:

1. Aspect-ratio correction on lift slope.
2. Flap deflection → shift in zero-lift AoA and stall angles.
3. Local air velocity: `surface_rotation.inverse() * world_air_velocity`
   (replaces `transform.InverseTransformDirection`).
   Discard Z component (spanwise flow).
4. Dynamic pressure: `0.5 * rho * v²`
5. Angle of attack: `atan2(vy, -vx)`
6. Coefficients via three-mode logic:
   - Low AoA (linear)
   - Stall (non-linear)
   - Blended transition (lerp)
7. Return `BiVector3 { force: lift + drag, torque: cross(rel_pos, force) + pitching_moment }`

All math is a 1:1 translation — every `Mathf.*` maps to `f32::*` or `Vec3::*`.

---

## Phase 3 — AircraftPhysics System

### `src/physics/aircraft_physics.rs`

Replaces both `AircraftPhysics.cs` and the Unity `Rigidbody` dependency.

**Entity setup** (spawned alongside the plane mesh):
```
RigidBody::Dynamic
  Mass(750.0)                        // kg, roughly Cessna 172
  Inertia::new(vec3(1285., 1825., 2667.))  // kg·m², Cessna 172 principal moments
  ExternalForce::default()
  ExternalTorque::default()
  AircraftRoot { thrust_max, throttle_percent }
  children: one AeroSurface entity per wing / elevator / rudder
```

**System `apply_aircraft_physics`** (fixed update, `PhysicsSchedule`):

Implements the trapezoidal (midpoint-prediction) integration from `AircraftPhysics.cs`:

```
1. Query RigidBody velocity + angular_velocity from Avian LinearVelocity / AngularVelocity.
2. frame_forces  = sum CalculateForces(vel, ang_vel)  over all AeroSurface children
3. vel_predicted = vel + dt*0.5 * (frame_forces.force + thrust + gravity) / mass
4. ang_predicted = ang_vel + dt*0.5 * inertia_inv * frame_forces.torque
5. pred_forces   = sum CalculateForces(vel_predicted, ang_predicted)
6. final = (frame_forces + pred_forces) * 0.5
7. Write into ExternalForce / ExternalTorque  (Avian accumulates these each tick)
8. Add thrust force separately along nose direction.
```

Avian's `ExternalForce` and `ExternalTorque` replace `rb.AddForce` / `rb.AddTorque`.
Avian reads `Inertia` automatically; no manual tensor rotation needed.

---

## Phase 4 — AirplaneController

### `src/physics/airplane_controller.rs`

Port of `AirplaneController.cs`. Reads keyboard input and writes `flap_angle`
on each `AeroSurface` child entity each fixed update.

```
W/S  → Pitch surfaces  (sensitivity ~0.2 rad)
A/D  → Roll  surfaces  (sensitivity ~0.2 rad)
Q/E  → Yaw   surfaces  (sensitivity ~0.2 rad)
F    → toggle Flap surfaces
=/−  → throttle up/down  (kept from current sim)
```

The controller queries all `AeroSurface` children of the plane entity,
matches on `input_type`, and calls `surface.set_flap_angle(input * sensitivity * multiplier)`.

---

## Phase 5 — Wiring & Cleanup

1. Remove `SimplePlanePhysics` and its system from `src/physics/simple.rs`
   (or keep as a backup).
2. Update `src/physics/mod.rs` to expose the new modules and register systems.
3. Update the plane spawn in `main.rs` / `plane.rs` to add Avian components
   and spawn AeroSurface child entities with Cessna-172-like configs:
   - Left wing, Right wing (Roll control)
   - Horizontal stabilizer / elevator (Pitch)
   - Vertical stabilizer / rudder (Yaw)
   - (Optional) Flaps on wings
4. Verify the debug HUD (`debug_hud.rs`) still reads speed/altitude correctly;
   Avian's `LinearVelocity` replaces `physics.velocity`.
5. Ensure `fog.rs` and camera systems still compile (they only touch `Transform`,
   so no changes expected).

---

## File Map

```
src/physics/
  mod.rs                    — register all systems, expose public types
  bi_vector3.rs             — BiVector3 struct
  aero_surface_config.rs    — AeroSurfaceConfig component
  aero_surface.rs           — AeroSurface component + CalculateForces logic
  aircraft_physics.rs       — apply_aircraft_physics system (trapezoidal integration)
  airplane_controller.rs    — input → flap angles + throttle
  simple.rs                 — (keep for reference, can delete after validation)
  PLAN.md                   — this file
```

---

## Risk / Notes

- **Inertia tensor**: Unity derives this from the mesh automatically. We will
  hardcode Cessna 172 published values (`Ixx=1285, Iyy=1825, Izz=2667` kg·m²).
  These can be tuned later via a config component.
- **Coordinate system**: Unity is left-handed (Y-up, Z-forward); Bevy is
  right-handed (Y-up, -Z-forward). The nose is `-transform.forward()` in Bevy.
  All cross products and local-space transforms need to account for this.
  The existing sim already handles this in `simple.rs`.
- **Air density**: Hardcode sea-level `1.2 kg/m³` for now; can be made
  altitude-dependent later using the ISA model.
- **Ground collision**: Avian will handle this once a `Collider` is added to
  the plane entity. The manual ground clamp in `simple.rs` can be removed.
