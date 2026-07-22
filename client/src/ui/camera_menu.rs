//! Camera window — toggled from the menu bar. A single Camera Mode section
//! for switching between Free, Orbit, and the fixed rigid-mount cameras
//! (nose, tail, wingtips, ...). Fullscreen and UI Scale live in the Graphics
//! menu instead (see `graphics_menu.rs`).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::{CameraMode, ChaseCam, FixedCameraMounts, FreeCam, TrackCam};
use crate::plane::Airplane;
use crate::ui::menu_bar::MenuBar;

pub struct CameraMenuPlugin;

impl Plugin for CameraMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_camera_menu.in_set(crate::ui::UiSet));
    }
}

fn draw_camera_menu(
    mut bar: ResMut<MenuBar>,
    mut mode: ResMut<CameraMode>,
    mounts: Res<FixedCameraMounts>,
    mut contexts: EguiContexts,
    mut cam_query: Query<(&Transform, &mut FreeCam, &mut ChaseCam, &mut TrackCam)>,
    plane_query: Query<&Transform, (With<Airplane>, Without<FreeCam>)>,
) -> Result {
    if !bar.camera { return Ok(()); }

    let ctx = contexts.ctx_mut()?;

    egui::Window::new("Camera")
        .open(&mut bar.camera)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(120.0, 48.0))
        .default_width(220.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading("Camera Mode");
            ui.separator();

            if ui.selectable_label(matches!(*mode, CameraMode::Orbit), "Orbit").clicked() {
                *mode = CameraMode::Orbit;
            }
            if ui.selectable_label(matches!(*mode, CameraMode::Chase), "Chase").clicked() {
                // Seed ChaseCam's look angles and plane-relative offset from
                // wherever the camera currently is — mirrors toggle_camera_mode.
                if let (Ok((tf, _, mut chase, _)), Ok(plane_tf)) = (cam_query.single_mut(), plane_query.single()) {
                    let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                    chase.yaw = yaw;
                    chase.pitch = pitch;
                    chase.offset = tf.translation - plane_tf.translation;
                }
                *mode = CameraMode::Chase;
            }
            if ui.selectable_label(matches!(*mode, CameraMode::Free), "Free Fly").clicked() {
                // Seed FreeCam's look angles from the current camera orientation
                // so switching in doesn't snap the view — mirrors toggle_camera_mode.
                if let Ok((tf, mut free, _, _)) = cam_query.single_mut() {
                    let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                    free.yaw = yaw;
                    free.pitch = pitch;
                }
                *mode = CameraMode::Free;
            }

            let fixed_selected = matches!(*mode, CameraMode::Fixed(_));
            egui::CollapsingHeader::new("Fixed Mounts")
                .default_open(fixed_selected)
                .show(ui, |ui| {
                    for (i, mount) in mounts.mounts.iter().enumerate() {
                        let selected = matches!(*mode, CameraMode::Fixed(idx) if idx == i);
                        if ui.selectable_label(selected, mount.name).clicked() {
                            *mode = CameraMode::Fixed(i);
                        }
                    }
                });
        });

    Ok(())
}
