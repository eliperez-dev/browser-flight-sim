//! In-game Pilot's Handbook — a toggleable egui window covering controls,
//! flight mechanics, engine operation, airports, and terrain generation.
//!
//! Toggle via the menu bar ("Handbook" button) or press **H**.
//! Opens by default on the first launch so new players immediately see the
//! control reference.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::ui::menu_bar::MenuBar;

pub struct PilotHandbookPlugin;

impl Plugin for PilotHandbookPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HandbookTab>()
            .add_systems(EguiPrimaryContextPass, draw_handbook.in_set(crate::ui::UiSet));
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Overview,
    Controls,
    Engine,
    FlightMechanics,
    HudReference,
    Airports,
    DayNight,
    Terrain,
}

/// Stores only the active tab; open/closed state lives in [`MenuBar`].
#[derive(Resource)]
pub struct HandbookTab {
    tab: Tab,
}

impl Default for HandbookTab {
    fn default() -> Self {
        Self { tab: Tab::Overview }
    }
}

fn draw_handbook(
    keys: Res<ButtonInput<KeyCode>>,
    mut bar: ResMut<MenuBar>,
    mut state: ResMut<HandbookTab>,
    mut contexts: EguiContexts,
) -> Result {
    if keys.just_pressed(KeyCode::KeyH) {
        bar.handbook = !bar.handbook;
    }
    if !bar.handbook {
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;

    egui::Window::new("Pilot's Handbook")
        .open(&mut bar.handbook)
        .order(egui::Order::Foreground)
        .default_pos(egui::pos2(16.0, 48.0))
        .default_width(480.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut state.tab, Tab::Overview,        "Overview");
                ui.selectable_value(&mut state.tab, Tab::Controls,        "Controls");
                ui.selectable_value(&mut state.tab, Tab::Engine,          "Engine");
                ui.selectable_value(&mut state.tab, Tab::FlightMechanics, "Flight");
                ui.selectable_value(&mut state.tab, Tab::HudReference,    "HUD");
                ui.selectable_value(&mut state.tab, Tab::Airports,        "Airports");
                ui.selectable_value(&mut state.tab, Tab::DayNight,        "Day / Night");
                ui.selectable_value(&mut state.tab, Tab::Terrain,         "Terrain");
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                match state.tab {
                    Tab::Overview        => tab_overview(ui),
                    Tab::Controls        => tab_controls(ui),
                    Tab::Engine          => tab_engine(ui),
                    Tab::FlightMechanics => tab_flight(ui),
                    Tab::HudReference    => tab_hud(ui),
                    Tab::Airports        => tab_airports(ui),
                    Tab::DayNight        => tab_day_night(ui),
                    Tab::Terrain         => tab_terrain(ui),
                }
            });
        });

    Ok(())
}

// ── Tab: Overview ─────────────────────────────────────────────────────────────

fn tab_overview(ui: &mut egui::Ui) {
    ui.heading("Welcome to the Flight Simulator");
    ui.add_space(6.0);
    ui.label("An infinite procedurally generated world to fly across. Every seed produces a different planet with its own terrain, biomes, coastlines, and hundreds of airports to find.");

    ui.add_space(10.0);
    ui.strong("Essential controls");
    ui.label("W / A / S / D   control surfaces (pitch and roll)");
    ui.label("+  /  -                  throttle up / down");
    ui.label("Arrow keys         camera look");
    ui.label("F                           switch camera mode");
    ui.label("G                           toggle physics gizmos");
    ui.label("H                           this handbook");

    ui.add_space(10.0);
    ui.strong("Quick start");
    ui.label("Engine starts off. Press I to start it, then use + to add throttle and W / S to fly.");
    ui.label("For the full keybind reference, go to the Controls tab.");
    ui.label("To understand the engine and mixture, see the Engine tab.");

    ui.add_space(10.0);
    ui.strong("What to do");
    ui.label("Open the F4 map to find airports nearby. Click one to set a Direct To waypoint and navigate to it. Use F3 to tune the flight model, world, weather, and lighting.");
}

// ── Tab: Controls ─────────────────────────────────────────────────────────────

fn tab_controls(ui: &mut egui::Ui) {
    ctrl_section(ui, "Flight", &[
        ("W / S",    "Pitch nose up / down"),
        ("A / D",    "Roll left / right"),
        ("E / Q",    "Rudder right / left  (yaw)"),
        ("B",        "Brakes  (hold)"),
        ("< / >",    "Flaps retract / extend"),
    ]);

    ui.add_space(10.0);
    ctrl_section(ui, "Engine", &[
        ("I",    "Start / stop engine"),
        ("+",    "Throttle up"),
        ("-",    "Throttle down"),
        ("L",    "Mixture rich  (more fuel)"),
        ("K",    "Mixture lean  (less fuel)"),
    ]);

    ui.add_space(10.0);
    ctrl_section(ui, "Lights", &[
        ("L",    "Toggle landing light"),
    ]);

    ui.add_space(10.0);
    ctrl_section(ui, "Camera", &[
        ("F",            "Toggle orbit / free camera"),
        ("Arrow keys",   "Look up / down / left / right"),
        ("W A S D",      "Free cam move forward / strafe"),
        ("E / Q",        "Free cam up / down"),
        ("Shift",        "Free cam speed boost"),
        ("Zoom In",      "Orbit camera zoom in"),
        ("Zoom Out",     "Orbit camera zoom out"),
    ]);

    ui.add_space(10.0);
    ctrl_section(ui, "Tools", &[
        ("H",       "This handbook"),
        ("F4",      "World map"),
        ("F3",      "Flight model debug panel"),
        ("G",       "Toggle physics gizmos"),
        ("P",       "Pause / unpause physics"),
        ("1",       "Toggle fog"),
    ]);
}

fn ctrl_section(ui: &mut egui::Ui, title: &str, binds: &[(&str, &str)]) {
    ui.strong(title);
    ui.add_space(2.0);
    for (key, desc) in binds {
        ui.horizontal(|ui| {
            // Fixed-width key column via a min-width label
            let key_label = egui::RichText::new(*key).monospace().strong();
            ui.add_sized([120.0, 0.0], egui::Label::new(key_label));
            ui.label(*desc);
        });
    }
}

// ── Tab: Engine ───────────────────────────────────────────────────────────────

fn tab_engine(ui: &mut egui::Ui) {
    ui.label("This is a piston engine with a carburetor. Three things control it:");
    ui.add_space(6.0);

    ui.strong("Throttle  ( + / - )");
    ui.label("Controls how much air and fuel enters the cylinders. Higher throttle means more RPM and more thrust. Reducing throttle reduces power and speed.");

    ui.add_space(6.0);
    ui.strong("Mixture  ( L / K )");
    ui.label("Controls the fuel-to-air ratio fed to the carburetor.");
    ui.add_space(2.0);
    ui.label("Rich (L)   more fuel relative to air. Best for takeoff and low altitude where air is dense.");
    ui.label("Lean (K)   less fuel relative to air. More efficient at cruise altitude where the air is thinner.");
    ui.add_space(2.0);
    ui.label("Around 70-80% is a good cruise setting. Too lean and the engine loses power or quits.");

    ui.add_space(6.0);
    ui.strong("Engine states");
    ui.add_space(2.0);
    ui.label("OFF         engine not running, no thrust");
    ui.label("CRANKING    starter engaged, spool-up in progress");
    ui.label("RUNNING     engine running, throttle and mixture active");

    ui.add_space(6.0);
    ui.strong("RPM");
    ui.label("Engine revolutions per minute. Reflects throttle and mixture. The propeller spin speed matches RPM.");
}

// ── Tab: Flight Mechanics ─────────────────────────────────────────────────────

fn tab_flight(ui: &mut egui::Ui) {
    ui.strong("Four forces");
    ui.label("Lift (wings up), Weight (gravity down), Thrust (engine forward), Drag (air resistance back). Flying is balancing all four.");

    ui.add_space(6.0);
    ui.strong("Lift");
    ui.label("Generated by the wings as air flows over them. Lift increases with speed and angle of attack (nose-up pitch). The HUD shows lift as a percentage of weight — at 100% you are weightless, above 100% you climb.");

    ui.add_space(6.0);
    ui.strong("Stall");
    ui.label("If the angle of attack is too steep, airflow over the wings separates and lift drops suddenly. The nose pitches down. Recover by releasing back pressure (let go of W) and adding throttle.");

    ui.add_space(6.0);
    ui.strong("Flaps");
    ui.label("Extend the trailing edge of the wing with < / >, increasing lift and drag simultaneously. Useful for slow-speed flight and landing, inefficient at cruise. Retract after takeoff.");

    ui.add_space(6.0);
    ui.strong("Bank to turn");
    ui.label("Roll the wings with A or D to bank. The aircraft will turn in the banked direction. Use the rudder (E / Q) to keep the nose coordinated.");

    ui.add_space(6.0);
    ui.strong("Ground effect");
    ui.label("Within about one wingspan of the ground the wing gets extra lift from the compressed air beneath it. Expect the plane to float further down the runway than you think.");

    ui.add_space(6.0);
    ui.strong("Flight assists");
    ui.label("Gentle auto-leveling and pitch damping are applied to keep handling accessible. Tunable or disableable in F3 under Flight Assists.");
}

// ── Tab: HUD Reference ────────────────────────────────────────────────────────

fn tab_hud(ui: &mut egui::Ui) {
    ui.label("The top-left overlay shows live flight data every frame.");
    ui.add_space(6.0);
    ctrl_section(ui, "", &[
        ("SPD / KTS",  "Airspeed in m/s and knots  (1 kt = 1.85 km/h)"),
        ("ALT",        "Altitude in metres above sea level"),
        ("GND",        "ON GROUND or AIRBORNE"),
        ("ENGINE",     "Off / Cranking / Running"),
        ("THROTTLE",   "Power lever position, 0-100%"),
        ("MIXTURE",    "Fuel-to-air ratio, 0-100%  (100 = fully rich)"),
        ("FLAPS",      "Flap deflection in degrees"),
        ("BRK",        "Brakes on / off"),
        ("RPM",        "Engine revolutions per minute"),
        ("LIFT",       "Lift as percent of weight needed to fly level"),
        ("THRUST",     "Engine thrust in Newtons"),
        ("DRAG",       "Total aerodynamic drag in Newtons"),
    ]);
}

// ── Tab: Airports ─────────────────────────────────────────────────────────────

fn tab_airports(ui: &mut egui::Ui) {
    ui.label("Airports are procedurally placed across the world. Open the F4 map to find them. Click one to see its name, elevation, runway heading, and length, and to set it as a Direct To navigation waypoint.");

    ui.add_space(8.0);
    ui.strong("Airport types");
    ui.add_space(2.0);
    for (name, desc) in &[
        ("Dirt Strip",      "Unpaved single strip. Short and narrow. Bush flying only."),
        ("Small GA",        "Paved general-aviation runway (~2000 m). Most common type and your spawn point."),
        ("Large Commuter",  "Longer paved runway (~3000 m). Handles faster regional aircraft."),
        ("Regional",        "Two parallel GA strips separated ~350 m. Higher traffic capacity."),
        ("Hub",             "Large multi-runway airport. Long runways and a major landmark on the map."),
    ] {
        ui.horizontal(|ui| {
            let label = egui::RichText::new(*name).monospace().strong();
            ui.add_sized([130.0, 0.0], egui::Label::new(label));
            ui.label(*desc);
        });
    }

    ui.add_space(8.0);
    ui.strong("Runway identifiers");
    ui.label("Each runway is numbered by its heading divided by 10. Runway 27 faces ~270 degrees (due west). Opposite ends differ by 18, e.g. 09/27.");

    ui.add_space(8.0);
    ui.strong("High-altitude airports");
    ui.label("Thinner air at altitude means less engine power and longer takeoff rolls. Lean the mixture aggressively when operating above 1000 m.");
}

// ── Tab: Day / Night ──────────────────────────────────────────────────────────

fn tab_day_night(ui: &mut egui::Ui) {
    ui.label("The world has a continuous day/night cycle driven by a simulated sun orbit. Time advances automatically and can be controlled in the F3 debug panel under Sky / Day-Night.");

    ui.add_space(8.0);
    ui.strong("Time of day");
    ui.label("The clock runs from 0.0 (midnight) to 1.0 (next midnight). Key points:");
    ui.add_space(2.0);
    ui.label("0.0     midnight");
    ui.label("0.25    sunrise");
    ui.label("0.5     noon");
    ui.label("0.75    sunset");
    ui.add_space(4.0);
    ui.label("The default start time is late afternoon (~15:36). You can scrub the time or pause it in F3.");

    ui.add_space(8.0);
    ui.strong("Sun and sky colour");
    ui.label("The sky transitions through night blue, dawn orange, midday white, sunset red, and back to night. The sun's light colour and intensity follow the same curve. At noon the lighting is bright and neutral; at sunrise and sunset it turns warm orange.");

    ui.add_space(8.0);
    ui.strong("Fog and haze");
    ui.label("When sky tinting is enabled (F3 > Sky) the fog colour blends with the sky colour. This gives a realistic haze effect where distant terrain picks up the orange of a sunset or the blue of twilight.");

    ui.add_space(8.0);
    ui.strong("Stars");
    ui.label("Stars appear at night and fade as the sun rises. Each star twinkles independently at a random speed. Stars are not real constellations but are seeded consistently.");

    ui.add_space(8.0);
    ui.strong("Aircraft lights");
    ui.label("Navigation lights (red left wingtip, green right, white tail) are always on. Strobes flash automatically. The landing light is toggled with L and casts a forward spotlight useful for night approaches. All light positions and intensities are adjustable in F3 > Lights.");

    ui.add_space(8.0);
    ui.strong("Sun orbit inclination");
    ui.label("The sun's orbit can be tilted (F3 > Sky > Orbit inclination). At 0 the sun rises due east and sets due west. A positive inclination tilts the orbit so the sun arcs to one side, giving a sense of latitude.");
}

// ── Tab: Terrain ──────────────────────────────────────────────────────────────

fn tab_terrain(ui: &mut egui::Ui) {
    ui.label("The world is infinite and generated on the fly. No two seeds produce the same world.");

    ui.add_space(6.0);
    ui.strong("Chunks");
    ui.label("The terrain is divided into 500 m x 500 m tiles called chunks. As you fly, nearby chunks are built and distant ones are discarded. Each chunk is a mesh whose vertex heights come from a noise function.");

    ui.add_space(6.0);
    ui.strong("LOD (Level of Detail)");
    ui.label("Chunks close to the aircraft get more mesh subdivisions. Farther chunks use fewer triangles. This keeps the framerate stable while distant terrain silhouettes remain visible. LOD bands are configurable in F3 > World Generation > Streaming & LOD.");

    ui.add_space(6.0);
    ui.strong("Noise and height");
    ui.label("Heights come from layered Perlin noise (fractal brownian motion). A base frequency sets the large-scale hills; octaves add finer detail. The result is scaled by the height_scale setting and shaped per biome.");

    ui.add_space(6.0);
    ui.strong("Climate and biomes");
    ui.label("Two noise fields (Temperature and Humidity) define a 2D climate map. Every point falls somewhere in that space. The four corners of the climate square are the four land biomes:");
    ui.add_space(4.0);
    for (biome, climate, desc) in &[
        ("Grasslands", "cold + dry",  "Gentle rolling hills, low relief"),
        ("Taiga",      "cold + wet",  "Tall mountains, dramatic peaks"),
        ("Desert",     "hot + dry",   "Near-flat, sandy, minimal relief"),
        ("Forest",     "hot + wet",   "Medium hills, dense green cover"),
    ] {
        ui.horizontal(|ui| {
            let b = egui::RichText::new(*biome).monospace().strong();
            ui.add_sized([90.0, 0.0], egui::Label::new(b));
            let c = egui::RichText::new(*climate).italics();
            ui.add_sized([100.0, 0.0], egui::Label::new(c));
            ui.label(*desc);
        });
    }
    ui.add_space(2.0);
    ui.label("Biomes blend smoothly at their boundaries.");

    ui.add_space(6.0);
    ui.strong("Oceans");
    ui.label("A continent-noise field decides where land exists. Points below the sea-level threshold become ocean. Coastlines blend gradually from water to beach terrain. Sea level is adjustable in F3 > Water.");

    ui.add_space(6.0);
    ui.strong("Seed");
    ui.label("A single 32-bit integer seeds everything: terrain, biomes, and airports. Change it in F3 > World Generation > Base to get a completely different world.");

    ui.add_space(6.0);
    ui.strong("Airports in terrain");
    ui.label("Airports sit on a sparse ~10 km grid. Each cell hashes its position against the seed to decide if an airport exists, what type, its runway heading, and its name. Terrain is flattened locally so the strip sits flush on the ground.");

    ui.add_space(6.0);
    ui.strong("Colliders");
    ui.label("Physics colliders are generated at a fixed resolution around the aircraft, independent of the visual LOD. Ground contact is always accurate even where the visual mesh is coarse.");
}
