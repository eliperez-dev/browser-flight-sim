//! In-game world map (debug). A toggleable egui window showing the terrain
//! around the player, nearby airports, a breadcrumb trail, and debug icons for
//! the aircraft and the terrain camera. Structured so more is easy to add:
//!
//! * Background layers are a [`MapLayer`] enum behind one `match` in
//!   [`render::bake`]; add a variant (plus [`MapLayer::ALL`]/[`MapLayer::name`])
//!   and it appears as a tab automatically.
//! * Each overlay is its own `draw_*` function in [`render`]; add another to
//!   draw new annotations.
//! * All view + interaction state lives in [`MapState`].
//!
//! Controls:
//!   * **F4** — open / close the map.
//!   * **tabs** — switch the background layer (Biome / Height).
//!   * **drag** — pan (stops following the plane).
//!   * **scroll** — zoom.
//!   * **left-click an airport** — identify it (ident, coords, elevation,
//!     runways) and "Direct to" it (draws a course line from the plane).
//!   * **right-click** — clear the selection and the active course.

mod render;

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, EguiTextureHandle, egui};

use crate::plane::Airplane;
use crate::terrain::{Airport, TerrainCamera, WorldGenerator, airport_name, airports_in_region, runway_ident};
use crate::water::WaterSettings;

use render::{TEX, View, half_span};

/// How long the breadcrumb trail remembers, and how often it samples. 10 min at
/// one sample every 3 s ⇒ at most 200 dots.
const TRAIL_SECONDS: f32 = 600.0;
const TRAIL_SAMPLE_SECONDS: f32 = 3.0;
const TRAIL_MAX: usize = (TRAIL_SECONDS / TRAIL_SAMPLE_SECONDS) as usize;

/// Which background layer the map draws.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MapLayer {
    /// Terrain surface colour (biomes + height shading) — matches the 3D world.
    #[default]
    Biome,
    /// Topographic greyscale: dark valleys → bright peaks, blue below sea level.
    Height,
    /// Flat colour per biome category (no shading) — clearest "what terrain is
    /// this" read.
    BiomeCategory,
}

impl MapLayer {
    /// Every layer, in tab order. Add a variant here (and a `name`) and it shows
    /// up as a tab automatically.
    const ALL: [MapLayer; 3] = [MapLayer::Biome, MapLayer::Height, MapLayer::BiomeCategory];

    fn name(self) -> &'static str {
        match self {
            MapLayer::Biome => "Biome",
            MapLayer::Height => "Height",
            MapLayer::BiomeCategory => "Category",
        }
    }
}

/// Pixel sizes for every map icon, surfaced as F3 debug sliders. Defaults match
/// the values the overlays were first authored with. `Clone`/`PartialEq` so the
/// debug panel can edit a copy and only write back on a real change.
#[derive(Resource, Clone, PartialEq)]
pub struct MapIconSettings {
    /// Plane marker: half-length and half-width of the body rectangle (px).
    pub plane_len: f32,
    pub plane_width: f32,
    /// Camera marker overall size (px); the triangle + FOV wedge scale from it.
    pub camera_size: f32,
    /// Zoomed-out airport symbol circle radius (px), and the runway-line stroke
    /// width used in both the zoomed-in strip and the symbol overlay (px).
    pub airport_circle: f32,
    pub runway_width: f32,
    /// Ring radius around the selected airport (px).
    pub selected_ring: f32,
    /// Breadcrumb dash half-length (px).
    pub breadcrumb_len: f32,
    /// Direct-to destination marker radius (px).
    pub waypoint_marker: f32,
    /// Font size for idents and the course label (px).
    pub label_font: f32,
}

impl Default for MapIconSettings {
    fn default() -> Self {
        Self {
            plane_len: 7.0,
            plane_width: 4.0,
            camera_size: 8.0,
            airport_circle: 7.0,
            runway_width: 4.0,
            selected_ring: 9.0,
            breadcrumb_len: 3.5,
            waypoint_marker: 6.0,
            label_font: 11.0,
        }
    }
}

/// One logged trail sample: where the plane was and which way it faced (XZ).
pub struct Breadcrumb {
    pos: Vec2,
    heading: Vec2,
}

/// Rolling history of the plane's track, sampled on a fixed interval and capped
/// at [`TRAIL_MAX`]. Logged regardless of whether the map is open, so the trail
/// already exists when you open it.
#[derive(Resource)]
pub struct BreadcrumbTrail {
    crumbs: VecDeque<Breadcrumb>,
    timer: Timer,
}

impl Default for BreadcrumbTrail {
    fn default() -> Self {
        Self {
            crumbs: VecDeque::with_capacity(TRAIL_MAX),
            timer: Timer::from_seconds(TRAIL_SAMPLE_SECONDS, TimerMode::Repeating),
        }
    }
}

/// Rows of the 256×256 background texture baked per frame. At 256 rows total,
/// 10 rows/frame means ~26 frames (~0.4 s at 60 fps) to fill a fresh view —
/// imperceptible for a map that only rebakes after panning settles.
const BAKE_ROWS_PER_FRAME: u32 = 20;

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
    /// The airport whose info panel is shown (clicked), if any.
    selected: Option<Airport>,
    /// The active "direct-to" destination, if any (drawn as a course line).
    waypoint: Option<Airport>,
    /// CPU-baked background, handed to egui as a texture.
    image: Handle<Image>,
    // --- snapshot of the view the current texture was baked for ---
    baked_center: Vec2,
    baked_world_per_texel: f32,
    baked_layer: MapLayer,
    /// False until the first bake completes, so the texture fills before drawing.
    baked: bool,
    /// Next texture row to bake (0 = idle / done, >0 = bake in progress).
    bake_row: u32,
    /// Center / zoom / layer frozen at the start of the current bake pass so the
    /// incremental rows are all sampled for the same view.
    bake_pass_center: Vec2,
    bake_pass_world_per_texel: f32,
    bake_pass_layer: MapLayer,
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
        app.init_resource::<BreadcrumbTrail>()
            .init_resource::<MapIconSettings>()
            .add_systems(Startup, setup_map)
            .add_systems(Update, (toggle_map, log_breadcrumbs))
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
        selected: None,
        waypoint: None,
        image: images.add(image),
        baked_center: Vec2::ZERO,
        baked_world_per_texel: 0.0,
        baked_layer: MapLayer::default(),
        baked: false,
        bake_row: 0,
        bake_pass_center: Vec2::ZERO,
        bake_pass_world_per_texel: 0.0,
        bake_pass_layer: MapLayer::default(),
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

/// Samples the plane's position + heading onto the trail every
/// [`TRAIL_SAMPLE_SECONDS`], dropping the oldest beyond [`TRAIL_MAX`]. Runs
/// always (not just while the map is open) so the trail is ready on open.
fn log_breadcrumbs(
    time: Res<Time>,
    mut trail: ResMut<BreadcrumbTrail>,
    plane_q: Query<&Transform, With<Airplane>>,
) {
    if !trail.timer.tick(time.delta()).just_finished() {
        return;
    }
    let Ok(tf) = plane_q.single() else { return };
    let fwd = tf.forward();
    trail.crumbs.push_back(Breadcrumb {
        pos: Vec2::new(tf.translation.x, tf.translation.z),
        heading: Vec2::new(fwd.x, fwd.z),
    });
    while trail.crumbs.len() > TRAIL_MAX {
        trail.crumbs.pop_front();
    }
}

/// Bakes the background if the view changed, then draws the window: layer tabs,
/// the texture, all overlays, the identify / direct-to panel, a key, and the
/// pan / zoom / click interaction.
#[allow(clippy::too_many_arguments)]
fn draw_map(
    mut contexts: EguiContexts,
    mut state: ResMut<MapState>,
    mut images: ResMut<Assets<Image>>,
    generator: Res<WorldGenerator>,
    trail: Res<BreadcrumbTrail>,
    icons: Res<MapIconSettings>,
    water: Res<WaterSettings>,
    plane_q: Query<&Transform, With<Airplane>>,
    cam_q: Query<&Transform, (With<TerrainCamera>, Without<Airplane>)>,
) -> Result {
    if !state.open {
        return Ok(());
    }

    // Follow the aircraft until the user pans away.
    let plane = plane_q.single().ok().map(|tf| {
        let fwd = tf.forward();
        (Vec2::new(tf.translation.x, tf.translation.z), Vec2::new(fwd.x, fwd.z))
    });
    if state.follow && let Some((pos, _)) = plane {
        state.center = pos;
    }

    // Start a new incremental bake pass whenever the view has settled and changed,
    // or when the generator / water level changed (one-offs, restart immediately).
    // While a gesture is in flight we hold off starting a new pass — the old
    // texture slides to follow the view instead, and we kick off once it settles.
    // Only reset bake_row when there isn't already a pass in progress, so we don't
    // keep restarting from row 0 every frame while baked=false.
    let force_restart = generator.is_changed() || water.is_changed();
    let pass_idle = state.bake_row >= render::TEX;
    if force_restart || (pass_idle && state.view_changed() && !state.interacting) {
        // Snapshot the view for this pass and immediately adopt it as the display
        // position so the partial texture is always shown at the correct location
        // rather than at the old baked position until the pass finishes.
        state.bake_pass_center = state.center;
        state.bake_pass_world_per_texel = state.world_per_texel;
        state.bake_pass_layer = state.layer;
        state.baked_center = state.center;
        state.baked_world_per_texel = state.world_per_texel;
        state.baked_layer = state.layer;
        state.bake_row = 0;
        // Wipe the texture so stale pixels from the previous view don't bleed
        // through while the new pass fills in row by row.
        if let Some(image) = images.get_mut(&state.image)
            && let Some(data) = image.data.as_mut() {
            data.fill(0);
        }
    }

    // Advance the incremental bake by up to BAKE_ROWS_PER_FRAME rows this frame.
    if state.bake_row < render::TEX
        && let Some(image) = images.get_mut(&state.image) {
        let start = state.bake_row;
        let end = (start + BAKE_ROWS_PER_FRAME).min(render::TEX);
        render::bake_rows(image, &generator, start, end, state.bake_pass_center, state.bake_pass_world_per_texel, state.bake_pass_layer, water.sea_level);
        state.bake_row = end;
        if state.bake_row >= render::TEX {
            state.baked = true;
        }
    }

    // Resolve the egui texture id (register on first use), then the context.
    let tex_id = match contexts.image_id(&state.image) {
        Some(id) => id,
        None => contexts.add_image(EguiTextureHandle::Strong(state.image.clone())),
    };
    let ctx = contexts.ctx_mut()?;

    let seed = generator.seed();
    let cam = cam_q
        .single()
        .ok()
        .map(|tf| {
            let fwd = tf.forward();
            (Vec2::new(tf.translation.x, tf.translation.z), Vec2::new(fwd.x, fwd.z))
        });

    egui::Window::new("Map")
        .default_pos(egui::pos2(16.0, 16.0))
        .resizable(false)
        .show(ctx, |ui| {
            // Layer tabs + the track-plane toggle. Tabs write the active layer (a
            // change triggers the deferred rebake via `view_changed`). The toggle
            // re-locks the map onto the plane; it reads `follow`, so it lights up
            // while tracking and dims the moment a pan clears `follow`.
            ui.horizontal(|ui| {
                for layer in MapLayer::ALL {
                    ui.selectable_value(&mut state.layer, layer, layer.name());
                }
                ui.separator();
                if ui
                    .selectable_label(state.follow, "⌖ Track")
                    .on_hover_text("Lock the map onto the plane (pan to unlock)")
                    .clicked()
                {
                    state.follow = true;
                }
            });
            ui.small("F4 close · drag pan · scroll zoom · click airport");

            let (response, painter) =
                ui.allocate_painter(egui::vec2(340.0, 340.0), egui::Sense::click_and_drag());
            let rect = response.rect;
            let view = View { rect, center: state.center, half: half_span(state.world_per_texel) };

            // Background texture, drawn at the world bounds it was *baked* for and
            // mapped through the current view: it fills the widget when the view
            // matches the bake, and slides/scales to track a gesture (bake held
            // off) so panning stays smooth. The painter clips to `rect`.
            if state.baked {
                let baked = View {
                    rect,
                    center: state.center,
                    half: TEX as f32 * state.baked_world_per_texel * 0.5,
                };
                let tmin = view.to_screen(state.baked_center - Vec2::splat(baked.half));
                let tmax = view.to_screen(state.baked_center + Vec2::splat(baked.half));
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

            // --- Overlays (cheap, redrawn every frame), back to front ---
            render::draw_breadcrumbs(&painter, &trail.crumbs, &icons, &view);

            let airports = airports_in_region(
                &generator,
                view.center.x - view.half,
                view.center.y - view.half,
                view.center.x + view.half,
                view.center.y + view.half,
            );
            render::draw_airports(
                &painter,
                &airports,
                seed,
                state.selected.as_ref().map(|s| s.cell),
                render::idents_visible(state.world_per_texel),
                &icons,
                &view,
            );

            if let (Some((ppos, _)), Some(wp)) = (plane, &state.waypoint) {
                let (wx, wz) = wp.pos();
                render::draw_waypoint(
                    &painter,
                    ppos,
                    Vec2::new(wx, wz),
                    &runway_ident(seed, wp.cell),
                    &icons,
                    &view,
                );
            }
            if let Some((pos, fwd)) = plane {
                render::draw_plane(&painter, pos, fwd, &icons, &view);
            }
            if let Some((pos, fwd)) = cam {
                render::draw_camera(&painter, pos, fwd, &icons, &view);
            }
            render::draw_ruler(&painter, &view);

            // --- Hover readout: world coords / elevation / biome under cursor ---
            if let Some(p) = response.hover_pos() {
                let w = view.to_world(p);
                let elev = generator.get_terrain_height(w.x, w.y);
                let biome = render::biome_name(generator.get_biome(w.x, w.y));
                render::draw_hover(&painter, p, w, elev, biome, rect);
            }

            // --- Click to select an airport / right-click to clear ---
            if response.clicked()
                && let Some(p) = response.interact_pointer_pos() {
                let mut best: Option<(f32, usize)> = None;
                for (i, ap) in airports.iter().enumerate() {
                    let (ax, az) = ap.pos();
                    let d = view.to_screen(Vec2::new(ax, az)).distance(p);
                    if d < 12.0 && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i));
                    }
                }
                state.selected = best.map(|(_, i)| airports[i].clone());
            }
            if response.secondary_clicked() {
                state.selected = None;
                state.waypoint = None;
            }

            // --- Identify panel + direct-to ---
            if let Some(ref a) = state.selected {
                let (ax, az) = a.pos();
                let marker = view.to_screen(Vec2::new(ax, az));
                if rect.contains(marker) {
                    let ident = runway_ident(seed, a.cell);
                    let full_name = airport_name(seed, a.cell, a.kind);
                    let primary = a.primary();
                    let (r1, r2) = primary.runway_numbers();
                    let strip_count = a.strips.len();
                    let mut set_wp = false;
                    egui::Area::new(egui::Id::new("map_airport_info"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(marker + egui::vec2(-40.0, 8.0))
                        .constrain_to(rect)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style().as_ref()).show(ui, |ui| {
                                ui.set_max_width(200.0);
                                ui.strong(&ident);
                                ui.label(&full_name);
                                ui.label(format!("Coords: {ax:.0}, {az:.0}"));
                                ui.label(format!("Elevation: {:.0} m", primary.elevation));
                                if strip_count > 1 {
                                    ui.label(format!("{strip_count}× {:.0} m rwy", primary.length));
                                } else {
                                    ui.label(format!("Rwy {r1:02}/{r2:02}  {:.0} m", primary.length));
                                }
                                ui.menu_button("Direct to ▾", |ui| {
                                    if ui.button(format!("Direct To {ident}")).clicked() {
                                        set_wp = true;
                                        ui.close();
                                    }
                                });
                            });
                        });
                    if set_wp {
                        state.waypoint = Some(a.clone());
                    }
                }
            }

            // --- Key / legend ---
            ui.collapsing("Key", |ui| {
                ui.label("▮ yellow  plane (nose = heading)");
                ui.label("▲ cyan    camera + FOV");
                ui.label("— magenta runway (oriented)");
                ui.label("· dashes  past track (last 10 min)");
                ui.label("⇢ orange  direct-to course");
            });

            // --- Interaction: pan and zoom ---
            let dragging = response.dragged();
            if dragging {
                let wpp = view.world_per_px();
                let d = response.drag_delta();
                state.center.x -= d.x * wpp;
                state.center.y -= d.y * wpp;
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
