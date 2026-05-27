//! In-game world map (debug). A toggleable egui window showing the terrain
//! around the player, the nearby airports, and debug icons for the aircraft and
//! the terrain camera. Structured so more is easy to add:
//!
//! * Background layers are a [`MapLayer`] enum behind one `match` in
//!   [`render::bake`]; add a variant (plus [`MapLayer::ALL`]/[`MapLayer::name`])
//!   and it appears as a tab automatically.
//! * Each overlay is its own `draw_*` function in [`render`]; add another to
//!   draw waypoints, a flight path, etc.
//! * All view + interaction state lives in [`MapState`].
//!
//! Controls:
//!   * **F4** — open / close the map.
//!   * **tabs** — switch the background layer (Biome / Height).
//!   * **drag** — pan (stops following the plane).
//!   * **scroll** — zoom.

mod render;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, EguiTextureHandle, egui};

use crate::plane::Airplane;
use crate::terrain::{TerrainCamera, WorldGenerator, runways_in_region};

use render::{TEX, half_span, world_to_screen};

/// Which background layer the map draws.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MapLayer {
    /// Terrain surface colour (biomes + height shading) — matches the 3D world.
    #[default]
    Biome,
    /// Topographic greyscale: dark valleys → bright peaks, blue below sea level.
    Height,
}

impl MapLayer {
    /// Every layer, in tab order. Add a variant here (and a `name`) and it shows
    /// up as a tab automatically.
    const ALL: [MapLayer; 2] = [MapLayer::Biome, MapLayer::Height];

    fn name(self) -> &'static str {
        match self {
            MapLayer::Biome => "Biome",
            MapLayer::Height => "Height",
        }
    }
}

/// All map view + interaction state. The background texture is rebaked only when
/// the *baked* fields fall out of step with the live view (see [`view_changed`]).
#[derive(Resource)]
pub struct MapState {
    open: bool,
    /// World (x, z) at the centre of the map.
    center: Vec2,
    /// Zoom: world metres covered by one texture texel. Larger = more area.
    world_per_texel: f32,
    layer: MapLayer,
    /// While set, the map re-centres on the aircraft each frame; a pan clears it.
    follow: bool,
    /// CPU-baked background, handed to egui as a texture.
    image: Handle<Image>,
    // --- snapshot of the view the current texture was baked for ---
    baked_center: Vec2,
    baked_world_per_texel: f32,
    baked_layer: MapLayer,
    /// False until the first bake, so the texture fills before the first draw.
    baked: bool,
    /// Whether the user dragged/zoomed on the *previous* frame. Baking is held
    /// off while this is set so a continuous pan doesn't resample 65k noise
    /// points every frame — the cached texture is slid/scaled to follow the view
    /// instead, and we rebake once the gesture settles.
    interacting: bool,
}

impl MapState {
    fn view_changed(&self) -> bool {
        !self.baked
            || self.layer != self.baked_layer
            || self.world_per_texel != self.baked_world_per_texel
            // Re-bake once the centre has drifted by half a texel — sub-pixel on
            // screen, so the cached texture never visibly lags the overlays.
            || self.center.distance(self.baked_center) > self.world_per_texel * 0.5
    }
}

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_map)
            .add_systems(Update, toggle_map)
            // Drawing (and the bake it may trigger) runs in the egui pass so it
            // targets the screen-space context, like the F3 debug panel.
            .add_systems(EguiPrimaryContextPass, draw_map);
    }
}

/// Creates the (initially black) background image and inserts [`MapState`].
fn setup_map(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = Image::new_fill(
        Extent3d { width: TEX, height: TEX, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        // Keep the CPU copy (MAIN_WORLD) so we can refill it on view changes, and
        // upload it (RENDER_WORLD) so egui can sample it.
        RenderAssetUsages::all(),
    );
    commands.insert_resource(MapState {
        open: false,
        center: Vec2::ZERO,
        // ~250 m/texel ⇒ a ~64 km span, so several of the 10 km-spaced strips show.
        world_per_texel: 250.0,
        layer: MapLayer::default(),
        follow: true,
        image: images.add(image),
        baked_center: Vec2::ZERO,
        baked_world_per_texel: 0.0,
        baked_layer: MapLayer::default(),
        baked: false,
        interacting: false,
    });
}

/// F4 opens / closes the map.
fn toggle_map(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<MapState>) {
    if keys.just_pressed(KeyCode::F4) {
        state.open = !state.open;
        // Re-opening re-arms follow, so a user who panned off can recover the
        // plane without a button (there are none yet).
        if state.open {
            state.follow = true;
        }
    }
}

/// Bakes the background if the view changed, then draws the window: texture +
/// airport / plane / camera overlays, with drag-to-pan and scroll-to-zoom.
fn draw_map(
    mut contexts: EguiContexts,
    mut state: ResMut<MapState>,
    mut images: ResMut<Assets<Image>>,
    generator: Res<WorldGenerator>,
    plane_q: Query<&Transform, With<Airplane>>,
    cam_q: Query<&Transform, (With<TerrainCamera>, Without<Airplane>)>,
) -> Result {
    if !state.open {
        return Ok(());
    }

    // Follow the aircraft until the user pans away.
    if state.follow {
        if let Ok(tf) = plane_q.single() {
            state.center = Vec2::new(tf.translation.x, tf.translation.z);
        }
    }

    // Rebake only when the view actually changed *and* the user isn't mid-gesture
    // (held off in the closure below): a continuous pan/zoom would otherwise
    // resample the whole texture every frame and stutter. A generator change
    // (new seed/scale) always rebakes — it's a one-off, not a per-frame gesture.
    if (state.view_changed() && !state.interacting) || generator.is_changed() {
        if let Some(image) = images.get_mut(&state.image) {
            render::bake(image, &generator, &state);
            state.baked_center = state.center;
            state.baked_world_per_texel = state.world_per_texel;
            state.baked_layer = state.layer;
            state.baked = true;
        }
    }

    // Resolve the egui texture id (register on first use), then the context.
    let tex_id = match contexts.image_id(&state.image) {
        Some(id) => id,
        None => contexts.add_image(EguiTextureHandle::Strong(state.image.clone())),
    };
    let ctx = contexts.ctx_mut()?;

    let half = half_span(&state);
    let center = state.center;

    egui::Window::new("Map")
        .default_pos(egui::pos2(16.0, 16.0))
        .resizable(false)
        .show(ctx, |ui| {
            // Layer tabs. Each is a selectable that writes the active layer; a
            // change triggers the deferred rebake via `view_changed`.
            ui.horizontal(|ui| {
                for layer in MapLayer::ALL {
                    ui.selectable_value(&mut state.layer, layer, layer.name());
                }
            });
            ui.small("F4 close · drag pan · scroll zoom");

            let (response, painter) =
                ui.allocate_painter(egui::vec2(340.0, 340.0), egui::Sense::click_and_drag());
            let rect = response.rect;

            // Draw the texture at the world bounds it was *baked* for, mapped
            // through the current view. When the view matches the bake it fills
            // the widget exactly; mid-gesture (bake held off) it slides/scales to
            // track the pan/zoom, so panning stays smooth without resampling. The
            // painter is clipped to `rect`, so any revealed edge is just trimmed.
            if state.baked {
                let baked_half = TEX as f32 * state.baked_world_per_texel * 0.5;
                let tmin = world_to_screen(
                    state.baked_center - Vec2::splat(baked_half),
                    rect,
                    center,
                    half,
                );
                let tmax = world_to_screen(
                    state.baked_center + Vec2::splat(baked_half),
                    rect,
                    center,
                    half,
                );
                painter.image(
                    tex_id,
                    egui::Rect::from_min_max(tmin, tmax),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
                egui::StrokeKind::Inside,
            );

            // --- Overlays (cheap, redrawn every frame) ---
            let runways = runways_in_region(
                &generator,
                center.x - half,
                center.y - half,
                center.x + half,
                center.y + half,
            );
            render::draw_airports(&painter, &runways, rect, center, half);

            if let Ok(tf) = plane_q.single() {
                let pos = Vec2::new(tf.translation.x, tf.translation.z);
                let fwd = tf.forward();
                render::draw_plane(&painter, pos, Vec2::new(fwd.x, fwd.z), rect, center, half);
            }
            if let Ok(tf) = cam_q.single() {
                let pos = Vec2::new(tf.translation.x, tf.translation.z);
                let fwd = tf.forward();
                render::draw_camera(&painter, pos, Vec2::new(fwd.x, fwd.z), rect, center, half);
            }

            // --- Interaction: pan and zoom ---
            let dragging = response.dragged();
            if dragging {
                let world_per_px = (2.0 * half) / rect.width();
                let d = response.drag_delta();
                state.center.x -= d.x * world_per_px;
                state.center.y -= d.y * world_per_px;
                state.follow = false;
            }
            let scroll = if response.hovered() {
                ui.input(|i| i.smooth_scroll_delta.y)
            } else {
                0.0
            };
            if scroll != 0.0 {
                // Exponential so each notch is a constant zoom ratio.
                let factor = (-scroll * 0.0015).exp();
                state.world_per_texel = (state.world_per_texel * factor).clamp(20.0, 4000.0);
            }
            // Hold off baking while a gesture is in flight; the frame after it
            // settles, `interacting` is clear and the deferred rebake runs once.
            state.interacting = dragging || scroll != 0.0;
        });

    Ok(())
}
