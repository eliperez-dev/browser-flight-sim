use avian3d::prelude::{AngularVelocity, CenterOfMass, LinearVelocity};
use bevy::prelude::*;

use crate::lights::{Beacon, LandingLight, NavLightLeft, NavLightRight, NavLightTail, StrobeLeft, StrobeRight, StrobeTail};
use crate::physics::aero_surface::AeroSurface;
use crate::physics::aircraft_physics::{AircraftRoot, ground_effect_factor};
use crate::physics::flight_config::FlightModelConfig;
use crate::physics::landing_gear::{GROUND_Y, gear_legs};
use crate::plane::{Airplane, PlaneVisual};
use crate::ui::menu_bar::{GizmosMode, MenuBar};

/// Mirrors `MenuBar::gizmos` into its own resource so gizmo/mesh systems
/// don't need to depend on the whole menu bar (and by extension egui) just
/// to read the current mode.
#[derive(Resource, Default)]
pub struct GizmosVisible(pub GizmosMode);

/// G key cycles the menu bar's gizmos mode (Off -> Outline -> Filled -> Off);
/// also syncs `GizmosVisible` from the bar every frame so clicking the menu
/// button has the same effect as pressing G.
pub fn toggle_gizmos(keys: Res<ButtonInput<KeyCode>>, mut bar: ResMut<MenuBar>, mut visible: ResMut<GizmosVisible>) {
    if keys.just_pressed(KeyCode::KeyG) {
        bar.gizmos = bar.gizmos.next();
    }
    visible.0 = bar.gizmos;
}

/// Hides the aircraft's visual mesh while in Filled gizmo mode (so the solid
/// aero-surface panels read clearly without the model occluding them), and
/// restores it otherwise.
pub fn apply_gizmos_mesh_visibility(
    visible: Res<GizmosVisible>,
    mut visual_q: Query<&mut Visibility, With<PlaneVisual>>,
) {
    if !visible.is_changed() {
        return;
    }
    let show_mesh = visible.0 != GizmosMode::Filled;
    for mut vis in &mut visual_q {
        *vis = if show_mesh { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Marker on the thin box mesh spawned as a child of each `AeroSurface`
/// entity for Filled gizmo mode — a solid stand-in for the surface's
/// span/chord rectangle (the flap portion, when present, gets its own child
/// so it can be tilted independently to show deflection).
#[derive(Component)]
pub struct FilledSurfaceMesh;

/// Marker on the flap portion of a filled surface (the trailing
/// `flap_fraction` of the chord), which is re-parented under a hinge pivot
/// so it can rotate by `flap_angle` independently of the fixed leading part.
#[derive(Component)]
pub struct FilledFlapPivot;

/// Marks an `AeroSurface` entity as already having its filled-mesh children
/// spawned, so `ensure_filled_surface_meshes` doesn't duplicate them on
/// every frame Filled mode is active.
#[derive(Component)]
pub struct FilledMeshBuilt;

/// Visual thickness of a filled surface panel (local units — metres × 10),
/// just enough to read as a solid plate rather than a zero-thickness sheet.
const FILLED_THICKNESS: f32 = 0.6;

/// Ensures every `AeroSurface` entity has a filled-mesh representation
/// (a fixed leading-edge box, plus — for control surfaces with a flap — a
/// hinge pivot carrying the trailing flap box), spawned lazily the first
/// time Filled mode is used rather than unconditionally at aircraft spawn.
#[allow(clippy::type_complexity)]
pub fn ensure_filled_surface_meshes(
    mut commands: Commands,
    visible: Res<GizmosVisible>,
    surface_q: Query<(Entity, &AeroSurface), Without<FilledMeshBuilt>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if visible.0 != GizmosMode::Filled {
        return;
    }

    for (entity, surface) in &surface_q {
        commands.entity(entity).insert(FilledMeshBuilt);
        let base_color = if surface.is_control_surface {
            Color::srgba(0.3, 0.9, 1.0, 0.9)
        } else {
            Color::srgba(0.2, 1.0, 0.4, 0.9)
        };
        let material = materials.add(StandardMaterial {
            base_color,
            unlit: true,
            cull_mode: None,
            ..default()
        });
        let flap_material = materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.55, 0.05, 0.9),
            unlit: true,
            cull_mode: None,
            ..default()
        });

        let has_flap = surface.is_control_surface && surface.config.flap_fraction > 0.0;
        let hc = surface.config.chord * 10.0 * 0.5; // half-chord, local units
        let fixed_chord = if has_flap {
            surface.config.chord * 10.0 * (1.0 - surface.config.flap_fraction)
        } else {
            surface.config.chord * 10.0
        };
        // Fixed part is centered between the leading edge and the hinge line.
        let fixed_center_x = hc - fixed_chord * 0.5;

        commands.entity(entity).with_children(|p| {
            p.spawn((
                Mesh3d(meshes.add(Cuboid::new(fixed_chord, FILLED_THICKNESS, surface.config.span * 10.0))),
                MeshMaterial3d(material),
                Transform::from_xyz(fixed_center_x, 0.0, 0.0),
                FilledSurfaceMesh,
                crate::camera::PIXEL_LAYER,
            ));

            if has_flap {
                let flap_chord = surface.config.chord * 10.0 * surface.config.flap_fraction;
                let hinge_x = hc - fixed_chord; // hinge line, local X relative to surface origin
                // Pivot sits ON the hinge line; the flap box is offset back
                // (-X) from the pivot by half its own chord so rotating the
                // pivot swings the flap about the hinge instead of its center.
                p.spawn((
                    Transform::from_xyz(hinge_x, 0.0, 0.0),
                    Visibility::Inherited,
                    FilledFlapPivot,
                )).with_children(|pivot| {
                    pivot.spawn((
                        Mesh3d(meshes.add(Cuboid::new(flap_chord, FILLED_THICKNESS, surface.config.span * 10.0))),
                        MeshMaterial3d(flap_material),
                        Transform::from_xyz(-flap_chord * 0.5, 0.0, 0.0),
                        FilledSurfaceMesh,
                        crate::camera::PIXEL_LAYER,
                    ));
                });
            }
        });
    }
}

/// Keeps filled-surface meshes visible only in Filled mode, and rotates each
/// flap pivot to `flap_angle` every frame so control deflection reads on the
/// solid geometry itself — same sign convention as `calculate_forces`:
/// positive `flap_angle` deflects the trailing edge down.
pub fn update_filled_surface_meshes(
    visible: Res<GizmosVisible>,
    surface_q: Query<&AeroSurface>,
    mut mesh_vis_q: Query<&mut Visibility, With<FilledSurfaceMesh>>,
    mut pivot_q: Query<(&ChildOf, &mut Transform), With<FilledFlapPivot>>,
) {
    let show = visible.0 == GizmosMode::Filled;
    for mut vis in &mut mesh_vis_q {
        *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    }
    if !show {
        return;
    }
    for (parent, mut tf) in &mut pivot_q {
        let Ok(surface) = surface_q.get(parent.parent()) else { continue };
        tf.rotation = Quat::from_rotation_z(surface.flap_angle);
    }
}

/// Draw the aero gizmos on top of the aircraft mesh instead of letting it
/// occlude them. `depth_bias = -1.0` renders gizmo lines in front of all
/// geometry (see Bevy's `GizmoConfig::depth_bias`), so the force/CoM arrows
/// always poke through the plane. Run once at startup.
pub fn setup_gizmo_config(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.depth_bias = -1.0;
}

#[allow(clippy::type_complexity)]
pub fn draw_aero_gizmos(
    visible: Res<GizmosVisible>,
    cfg: Res<FlightModelConfig>,
    aircraft_q: Query<
        (&Transform, &LinearVelocity, &AngularVelocity, &CenterOfMass, &Children, &AircraftRoot),
        With<Airplane>,
    >,
    surface_q: Query<(&AeroSurface, &Transform), Without<Airplane>>,
    mut gizmos: Gizmos,
) {
    if visible.0 == GizmosMode::Off {
        return;
    }


    let Ok((tf, lin_vel, ang_vel, com, children, root)) = aircraft_q.single() else {
        return;
    };

    // CoM world position derived from the root's interpolated Transform (updated every frame
    // by TransformInterpolation), not GlobalTransform (only updated on physics steps).
    // CenterOfMass is in metres (unscaled), matching Avian's
    // global_center_of_mass = position + rotation * center_of_mass — so do NOT
    // apply tf.scale here, or the offset (and its shift under load) shrinks 10×.
    let com_world = tf.translation + tf.rotation * com.0;

    gizmos.sphere(Isometry3d::from_translation(com_world), 0.3, Color::WHITE);

    let vel = lin_vel.0;
    if vel.length() > 0.1 {
        gizmos.arrow(com_world, com_world + vel * 0.2, Color::srgb(0.0, 1.0, 1.0));
    }

    let nose = tf.rotation * Vec3::Z;
    let thrust_n = cfg.thrust_max * root.throttle_percent * 5.0;
    gizmos.arrow(
        com_world,
        com_world + nose * (thrust_n / 2000.0),
        Color::srgb(0.2, 0.4, 1.0),
    );

    gizmos.arrow(com_world, com_world + Vec3::NEG_Y * 3.0, Color::srgb(1.0, 0.2, 0.2));

    // Landing-gear struts: a line from each mount down to the wheel at full
    // extension, with a sphere at the contact point. Mounts are in metres in the
    // body frame (like the CoM), so reconstruct world position the same way —
    // tf.translation + tf.rotation * mount — *without* tf.scale. The wheel hangs
    // `gear_rest_length` along body-down (−Y); this is exactly where the strut in
    // landing_gear.rs starts looking for the ground, so the spheres mark where
    // the aircraft will touch down.
    {
        let strut_color = Color::srgb(0.8, 0.4, 1.0); // violet
        let down = tf.rotation * Vec3::NEG_Y;
        for leg in gear_legs(&cfg) {
            let mount_world = tf.translation + tf.rotation * leg.mount;
            let wheel_world = mount_world + down * leg.rest_length;
            gizmos.line(mount_world, wheel_world, strut_color);
            gizmos.sphere(Isometry3d::from_translation(wheel_world), 0.2, strut_color);
        }
    }

    // Fuselage drag box: a cuboid at the CoM whose extents are the per-axis
    // Cd·A (X=flank, Y=belly/top, Z=nose). The thin forward dimension vs. the
    // broad side/vertical faces visualises why pulls and skids bleed energy but
    // level cruise stays slippery. Oriented with the body, sized in metres.
    // {
    //     let h = cfg.fuselage_drag * 0.5 * 0.2; // half-extents
    //     let drag_color = Color::srgb(1.0, 0.0, 0.8);
    //     // 8 corners in body frame, transformed to world via the body rotation.
    //     let corner = |sx: f32, sy: f32, sz: f32| {
    //         com_world + tf.rotation * Vec3::new(sx * h.x, sy * h.y, sz * h.z)
    //     };
    //     let signs = [-1.0_f32, 1.0];
    //     for &sy in &signs {
    //         for &sz in &signs {
    //             gizmos.line(corner(-1.0, sy, sz), corner(1.0, sy, sz), drag_color); // X edges
    //         }
    //     }
    //     for &sx in &signs {
    //         for &sz in &signs {
    //             gizmos.line(corner(sx, -1.0, sz), corner(sx, 1.0, sz), drag_color); // Y edges
    //         }
    //     }
    //     for &sx in &signs {
    //         for &sy in &signs {
    //             gizmos.line(corner(sx, sy, -1.0), corner(sx, sy, 1.0), drag_color); // Z edges
    //         }
    //     }
    // }

    // Newtons → metres of arrow. Tuned so a wing at cruise reads ~1.5 m and a
    // hard pull grows it visibly, capped so high-g loads don't fill the screen.
    const FORCE_TO_M: f32 = 0.0004;

    // Aerodynamic center (the whole-aircraft neutral point): the point about
    // which the pitching moment does NOT change with angle of attack. It's where
    // the *increment* of aero force with AoA acts, so we find it by finite
    // difference — sampling every surface at the current freestream and at a
    // freestream nudged by `D_ALPHA`, then locating where ΔF acts:
    //   AC = CoM + (ΔF × ΔM) / |ΔF|².
    // Unlike the center of pressure, ΔF (the lift-curve-slope force) stays large
    // and steady in trim, so the AC sits still; it only shifts near stall where
    // the lift slope collapses. The rotational airflow term is excluded on
    // purpose — the AC is a pure AoA response, not a rate/damping effect.
    let pitch_axis = tf.rotation * Vec3::X; // body X = pitch axis
    let d_alpha = 2.0_f32.to_radians();
    let base_wind = -vel; // freestream relative to the aircraft
    let pert_wind = Quat::from_axis_angle(pitch_axis, d_alpha) * base_wind;
    let mut f_base = Vec3::ZERO;
    let mut m_base = Vec3::ZERO;
    let mut f_pert = Vec3::ZERO;
    let mut m_pert = Vec3::ZERO;

    // Vectors mode shows only whole-aircraft arrows/markers (already drawn
    // above) plus the aerodynamic center below — no per-surface panels, force
    // arrows, or AoA indicators. The per-surface force/moment sampling below
    // still needs to run in every non-Off mode, though, since the AC marker
    // depends on it.
    let show_surfaces = visible.0 != GizmosMode::Vectors;

    for child in children {
        let Ok((surface, local_tf)) = surface_q.get(*child) else {
            continue;
        };

        // Reconstruct world position from root's interpolated Transform + child local Transform.
        // child local_tf.translation is in the parent's local space (scale 0.1 already applies).
        let pos = tf.transform_point(local_tf.translation);
        let rot = tf.rotation * local_tf.rotation;

        // Surface geometry rectangle: span along local Z, chord along local X.
        // Half-extents in world space.
        let c = &surface.config;
        let hc = c.chord * 0.5; // half-chord

        if show_surfaces {
            let hs = c.span * 0.5; // half-span
            let span_dir = rot * Vec3::Z;  // local Z = span
            let chord_dir = rot * Vec3::X; // local X = chord
            let surface_color = if surface.is_control_surface {
                Color::srgba(0.3, 0.9, 1.0, 0.8)
            } else {
                Color::srgba(0.2, 1.0, 0.4, 0.8)
            };
            // Panel outline. Leading edge is +chord_dir (nose-ward), trailing edge
            // is -chord_dir. For control surfaces with a flap, the trailing
            // `flap_fraction` of the chord is drawn as its own hinged quad that
            // rotates about the hinge line by `flap_angle`, so the deflection is
            // shown directly on the panel geometry instead of a separate pointer
            // that can visually disagree with the panel's own orientation.
            let panel_up = rot * Vec3::Y;
            let has_flap = surface.is_control_surface && c.flap_fraction > 0.0;
            // Hinge line sits `flap_fraction` of the chord back from the leading
            // edge — i.e. the FIXED part is `(1 - flap_fraction)` of the chord,
            // matching `ensure_filled_surface_meshes`'s `fixed_chord` (this had
            // been using `flap_fraction` directly, giving the fixed part only
            // `flap_fraction` of the chord instead of the rest of it).
            let hinge_x = if has_flap { hc - c.chord * (1.0 - c.flap_fraction) } else { -hc };

            // Fixed part: leading edge to the hinge line (the whole panel if no flap).
            let fixed_corners = [
                pos + span_dir * hs + chord_dir * hc,
                pos - span_dir * hs + chord_dir * hc,
                pos - span_dir * hs + chord_dir * hinge_x,
                pos + span_dir * hs + chord_dir * hinge_x,
            ];
            for i in 0..4 {
                gizmos.line(fixed_corners[i], fixed_corners[(i + 1) % 4], surface_color);
            }
            gizmos.line(fixed_corners[0], fixed_corners[2], Color::srgba(surface_color.to_srgba().red, surface_color.to_srgba().green, surface_color.to_srgba().blue, 0.3));
            gizmos.line(fixed_corners[1], fixed_corners[3], Color::srgba(surface_color.to_srgba().red, surface_color.to_srgba().green, surface_color.to_srgba().blue, 0.3));

            // Hinged flap part: hinge line back to the trailing edge, rotated
            // about the hinge by `flap_angle`. Positive flap_angle = more lift =
            // trailing edge deflects down (toward -panel_up), matching the sign
            // convention `calculate_forces` uses for zero-lift AoA.
            if has_flap {
                let flap_color = if surface.flap_angle.abs() > 0.005 {
                    Color::srgb(1.0, 0.55, 0.05)
                } else {
                    surface_color
                };
                let flap_chord = c.chord * c.flap_fraction;
                let deflect_dir = -chord_dir * surface.flap_angle.cos() - panel_up * surface.flap_angle.sin();
                let hinge_l = pos + span_dir * hs + chord_dir * hinge_x;
                let hinge_r = pos - span_dir * hs + chord_dir * hinge_x;
                let trail_l = hinge_l + deflect_dir * flap_chord;
                let trail_r = hinge_r + deflect_dir * flap_chord;
                gizmos.line(hinge_l, hinge_r, flap_color);
                gizmos.line(hinge_l, trail_l, flap_color);
                gizmos.line(trail_l, trail_r, flap_color);
                gizmos.line(trail_r, hinge_r, flap_color);
                gizmos.line(hinge_l, trail_r, Color::srgba(flap_color.to_srgba().red, flap_color.to_srgba().green, flap_color.to_srgba().blue, 0.3));
                gizmos.line(hinge_r, trail_l, Color::srgba(flap_color.to_srgba().red, flap_color.to_srgba().green, flap_color.to_srgba().blue, 0.3));
            }
        }

        // Actual aerodynamic force this surface produces right now.
        let rel_pos = pos - com_world;
        let world_air_vel = -vel - ang_vel.0.cross(rel_pos);
        let ge = ground_effect_factor(pos.y - GROUND_Y, cfg.ground_effect_span, cfg.ground_effect_strength);

        // AC sampling — needed in every non-Off mode, since Vectors mode
        // still shows the aerodynamic center marker.
        let fb = surface.calculate_forces(base_wind, cfg.air_density, rel_pos, rot, ge).force;
        let fp = surface.calculate_forces(pert_wind, cfg.air_density, rel_pos, rot, ge).force;
        f_base += fb;
        m_base += rel_pos.cross(fb);
        f_pert += fp;
        m_pert += rel_pos.cross(fp);

        if show_surfaces {
            let force = surface
                .calculate_forces(world_air_vel, cfg.air_density, rel_pos, rot, ge)
                .force;

            // Force arrow (lift+drag resultant)
            let arrow_len = (force.length() * FORCE_TO_M).min(6.0);
            let lift_color = if surface.is_control_surface {
                Color::srgb(0.4, 1.0, 0.4)
            } else {
                Color::srgb(0.0, 0.8, 0.0)
            };
            gizmos.arrow(pos, pos + force.normalize_or_zero() * arrow_len, lift_color);

            // AoA indicator: only shown when actually flying.
            if vel.length() > 5.0 {
                let airflow_dir = world_air_vel.normalize_or_zero();
                let chord_len = hc.min(1.2) * 2.2;
                gizmos.arrow(pos, pos + airflow_dir * chord_len * 1.3, Color::srgba(1.0, 1.0, 1.0, 0.7));
            }

            gizmos.line(com_world, pos, Color::srgba(1.0, 1.0, 1.0, 0.15));
        }
    }

    // Draw the aerodynamic center once there's a meaningful lift-curve response
    // (skip on the ground / near zero airspeed / deep stall). The yellow↔white
    // gap is the static margin: AC aft of the CoM (toward the tail) is
    // pitch-stable, AC ahead of it is unstable.
    let d_force = f_pert - f_base;
    let d_moment = m_pert - m_base;
    if vel.length() > 5.0 && d_force.length() > 1.0 {
        let ac_rel = (d_force.cross(d_moment) / d_force.length_squared()).clamp_length_max(10.0);
        let ac_world = com_world + ac_rel;
        let ac_color = Color::srgb(1.0, 0.9, 0.0); // yellow
        gizmos.sphere(Isometry3d::from_translation(ac_world), 0.25, ac_color);
        gizmos.line(com_world, ac_world, ac_color);
    }
}

/// Draws a small sphere at each exterior light's world position (color-coded
/// by type) and a forward arrow for the landing spotlight. Runs under the same
/// G toggle as the aero gizmos.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn draw_light_gizmos(
    visible: Res<GizmosVisible>,
    aircraft_q: Query<&Transform, With<Airplane>>,
    nav_l_q:    Query<&Transform, (With<NavLightLeft>,  Without<Airplane>)>,
    nav_r_q:    Query<&Transform, (With<NavLightRight>, Without<Airplane>)>,
    nav_t_q:    Query<&Transform, (With<NavLightTail>,  Without<Airplane>)>,
    str_l_q:    Query<&Transform, (With<StrobeLeft>,    Without<Airplane>)>,
    str_r_q:    Query<&Transform, (With<StrobeRight>,   Without<Airplane>)>,
    str_t_q:    Query<&Transform, (With<StrobeTail>,    Without<Airplane>)>,
    beacon_q:   Query<&Transform, (With<Beacon>,        Without<Airplane>)>,
    landing_q:  Query<&Transform, (With<LandingLight>,  Without<Airplane>)>,
    mut gizmos: Gizmos,
) {
    if visible.0 == GizmosMode::Off { return; }
    let Ok(root_tf) = aircraft_q.single() else { return };

    // Convert a child's local Transform to world position using the aircraft
    // root's interpolated Transform (same approach as the aero gizmos).
    let to_world = |local_tf: &Transform| root_tf.transform_point(local_tf.translation);

    let r = 0.18_f32; // sphere radius for all light gizmos

    for tf in &nav_l_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::srgb(1.0, 0.05, 0.05));
    }
    for tf in &nav_r_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::srgb(0.05, 1.0, 0.15));
    }
    for tf in &nav_t_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::WHITE);
    }
    for tf in &str_l_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::srgb(0.8, 0.8, 1.0));
    }
    for tf in &str_r_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::srgb(0.8, 0.8, 1.0));
    }
    for tf in &str_t_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::srgb(0.8, 0.8, 1.0));
    }
    for tf in &beacon_q {
        gizmos.sphere(Isometry3d::from_translation(to_world(tf)), r, Color::srgb(1.0, 0.1, 0.1));
    }

    // Landing light: sphere + an arrow showing the beam direction.
    // SpotLight shines along local -Z of the child; the child's rotation
    // already points it forward-down, so world_dir = root_rot * child_rot * (-Z).
    for tf in &landing_q {
        let world_pos = to_world(tf);
        let beam_dir = (root_tf.rotation * tf.rotation) * Vec3::NEG_Z;
        gizmos.sphere(Isometry3d::from_translation(world_pos), r, Color::srgb(1.0, 1.0, 0.6));
        gizmos.arrow(world_pos, world_pos + beam_dir * 4.0, Color::srgb(1.0, 1.0, 0.6));
    }
}
