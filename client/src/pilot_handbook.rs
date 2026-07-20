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
    Instruments,
    Airports,
    DayNight,
    Terrain,
    Multiplayer,
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

    // Centre the window on first open only (an explicit `default_pos`, not an
    // `anchor` — `anchor` would make it immovable, which breaks dragging on
    // this resizable window). Estimated against a typical ~480x520 window
    // size since egui doesn't know the real size until after the first show.
    let screen = ctx.content_rect();
    let default_pos = egui::pos2(
        (screen.width() / 2.0 - 240.0).max(8.0),
        (screen.height() / 2.0 - 260.0).max(8.0),
    );

    egui::Window::new("Pilot's Handbook")
        .open(&mut bar.handbook)
        .order(egui::Order::Tooltip)
        .default_pos(default_pos)
        .default_width(480.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut state.tab, Tab::Overview,        "Overview");
                ui.selectable_value(&mut state.tab, Tab::Controls,        "Controls");
                ui.selectable_value(&mut state.tab, Tab::Engine,          "Engine");
                ui.selectable_value(&mut state.tab, Tab::FlightMechanics, "Flight");
                ui.selectable_value(&mut state.tab, Tab::Instruments,     "Instruments");
                ui.selectable_value(&mut state.tab, Tab::Airports,        "Airports");
                ui.selectable_value(&mut state.tab, Tab::DayNight,        "Day / Night");
                ui.selectable_value(&mut state.tab, Tab::Terrain,         "Terrain");
                ui.selectable_value(&mut state.tab, Tab::Multiplayer,     "Multiplayer");
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                match state.tab {
                    Tab::Overview        => tab_overview(ui),
                    Tab::Controls        => tab_controls(ui),
                    Tab::Engine          => tab_engine(ui),
                    Tab::FlightMechanics => tab_flight(ui),
                    Tab::Instruments     => tab_instruments(ui),
                    Tab::Airports        => tab_airports(ui),
                    Tab::DayNight        => tab_day_night(ui),
                    Tab::Terrain         => tab_terrain(ui),
                    Tab::Multiplayer     => tab_multiplayer(ui),
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
    ui.strong("Quick start");
    ui.label("Engine starts off — press I, then + for throttle and W / S to fly. Full keybinds are in the Controls tab.");

    ui.add_space(10.0);
    ui.strong("If you crash");
    ui.label("Press R, or use the Reset Plane to Runway button in My Plane, to get back on the runway.");

    ui.add_space(10.0);
    ui.strong("Menu bar");
    ctrl_section(ui, "", &[
        ("Map",         "World map, airports, Direct To navigation  (F4)"),
        ("Handbook",    "This window  (H)"),
        ("World",       "Weather and terrain settings"),
        ("My Plane",    "Engine, loadout, and flight-assist settings"),
        ("Camera",      "Camera mode and fixed-mount settings"),
        ("Multiplayer", "Server browser, hosting, and player list — see the Multiplayer tab"),
        ("Gizmos",      "Physics debug overlays  (G)"),
        ("Dev Tools",   "Flight model, world generation, and debug HUD  (F3)"),
    ]);
}

// ── Tab: Controls ─────────────────────────────────────────────────────────────

fn tab_controls(ui: &mut egui::Ui) {
    ctrl_section(ui, "Flight", &[
        ("W / S",    "Pitch nose up / down"),
        ("A / D",    "Roll left / right"),
        ("E / Q",    "Rudder right / left  (yaw)"),
        ("B",        "Brakes  (hold)"),
        ("< / >",    "Flaps retract / extend"),
        ("R",        "Reset to runway  (after a crash)"),
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
        ("N",    "Toggle landing light  (nav, strobe, and beacon have on-screen switches instead)"),
    ]);

    ui.add_space(10.0);
    ctrl_section(ui, "Camera", &[
        ("F",            "Cycle Orbit -> Chase -> Free camera"),
        ("Arrow keys",   "Look up / down / left / right"),
        ("W A S D",      "Free/Chase cam move forward / strafe"),
        ("E / Q",        "Free/Chase cam up / down"),
        ("Shift",        "Free/Chase cam speed boost"),
        ("[ / ]",        "Orbit camera zoom in / out"),
        ("1-4",          "Jump to fixed camera mount  (nose, tail, left wing, right wing)"),
        ("F11",          "Toggle fullscreen"),
    ]);

    ui.add_space(10.0);
    ctrl_section(ui, "Tools", &[
        ("H",       "This handbook"),
        ("F4",      "World map"),
        ("F3",      "Dev Tools  (flight model, world gen, debug HUD)"),
        ("G",       "Toggle physics gizmos"),
        ("P",       "Pause / unpause physics"),
    ]);

    ui.add_space(10.0);
    ui.label(egui::RichText::new("Multiplayer has no dedicated hotkeys — open it from the menu bar.").italics());
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
    ui.label("A piston engine with a carburetor. Throttle and mixture control it.");
    ui.add_space(6.0);

    ui.strong("Throttle  ( + / - )");
    ui.label("More throttle means more RPM and more thrust.");

    ui.add_space(6.0);
    ui.strong("Mixture  ( L / K )");
    ui.label("Rich (L): more fuel relative to air, best for takeoff and low altitude. Lean (K): less fuel, more efficient at altitude where the air is thinner. The ideal mixture leans out gradually as you climb — the correct setting scales with altitude, not a fixed number. Too lean or too rich and the engine loses power; far enough either way and it quits.");

    ui.add_space(6.0);
    ui.strong("Engine states");
    ui.add_space(2.0);
    ui.label("OFF         engine not running, no thrust");
    ui.label("CRANKING    starter engaged, spool-up in progress");
    ui.label("RUNNING     engine running, throttle and mixture active");
    ui.add_space(2.0);
    ui.label("The starter (I) won't catch if the mixture is leaned too far — rich it up first if it won't start.");

    ui.add_space(6.0);
    ui.strong("RPM");
    ui.label("Reflects throttle and mixture. The propeller spin speed matches RPM.");
}

// ── Tab: Flight Mechanics ─────────────────────────────────────────────────────

fn tab_flight(ui: &mut egui::Ui) {
    ui.strong("Four forces");
    ui.label("Lift (wings up), Weight (gravity down), Thrust (engine forward), Drag (air resistance back). Flying is balancing all four.");

    ui.add_space(6.0);
    ui.strong("Stall");
    ui.label("Too steep an angle of attack and airflow over the wings separates — lift drops and the nose pitches down. Recover by releasing back pressure (let go of W) and adding throttle.");

    ui.add_space(6.0);
    ui.strong("Flaps");
    ui.label("Extend with < / >: more lift and drag, useful for slow-speed flight and landing. Retract after takeoff.");

    ui.add_space(6.0);
    ui.strong("Bank to turn");
    ui.label("Roll with A / D to bank; use the rudder (E / Q) to keep the turn coordinated.");

    ui.add_space(6.0);
    ui.strong("Ground effect");
    ui.label("Within about one wingspan of the ground the wing gets extra lift. Expect the plane to float further down the runway than you'd think.");

    ui.add_space(6.0);
    ui.strong("Crashing");
    ui.label("A hard landing or ground strike outside the gear will crash the aircraft. Press R (or My Plane > Reset Plane to Runway) to recover.");

    ui.add_space(6.0);
    ui.strong("Flight assists");
    ui.label("Gentle auto-leveling and pitch damping keep handling accessible. Tune or disable them in Dev Tools (F3) under Flight Assists.");
}

// ── Tab: Instruments ──────────────────────────────────────────────────────────

fn tab_instruments(ui: &mut egui::Ui) {
    ui.label("The instrument panel is always on screen in Orbit camera mode (hidden in Chase and Free — press F to cycle modes).");
    ui.add_space(6.0);

    ui.strong("Flight instruments");
    ctrl_section(ui, "", &[
        ("Artificial horizon", "Pitch ladder and bank angle, centre of the panel"),
        ("Airspeed tape",      "Knots; green band is normal cruise, amber near stall/overspeed"),
        ("Altitude tape",      "Feet above sea level"),
        ("VSI",                "Vertical speed, feet per minute"),
        ("Heading compass",    "Magnetic heading strip along the top"),
        ("Barometric readout", "Local pressure in inHg, derived from altitude"),
    ]);

    ui.add_space(8.0);
    ui.strong("Engine and controls");
    ctrl_section(ui, "", &[
        ("RPM gauge",       "Arc gauge with redline"),
        ("Throttle lever",  "Drag, or use + / -"),
        ("Mixture lever",   "Drag, or use L / K"),
        ("Flap lever",      "Drag, or use < / >"),
        ("Trim lever",      "Elevator trim, drag only"),
    ]);

    ui.add_space(8.0);
    ui.strong("Switches");
    ctrl_section(ui, "", &[
        ("NAV / STRB",  "Navigation and strobe light master switches"),
        ("LANDING",     "Same as the N key"),
        ("BRAKE / PARK","Brake status; PARK holds the brakes without holding B"),
    ]);

    ui.add_space(8.0);
    ui.label(egui::RichText::new("A separate, more detailed debug HUD (speed, lift, thrust, drag, and more as raw numbers) is available in Dev Tools (F3).").color(egui::Color32::from_rgb(139, 154, 181)).small());
}

// ── Tab: Airports ─────────────────────────────────────────────────────────────

fn tab_airports(ui: &mut egui::Ui) {
    ui.label("Airports are procedurally placed across the world. Open the F4 map to find them. Click one to see its name, elevation, runway heading, and length, and to set it as a Direct To navigation waypoint.");

    ui.add_space(8.0);
    ui.strong("Airport types");
    ui.add_space(2.0);
    for (name, desc) in &[
        ("Dirt Strip",      "Unpaved single strip, 400-900 m. Bush flying only."),
        ("Small GA",        "Paved general-aviation runway (~2000 m). Most common type and your spawn point."),
        ("Large Commuter",  "Longer paved runway (~3200 m). Handles faster regional aircraft."),
        ("Regional",        "Two parallel GA strips ~70 m apart. Higher traffic capacity."),
        ("Hub",             "Two parallel strips ~180 m apart. Long runways and a major landmark on the map."),
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
    ui.label("A continuous day/night cycle driven by a simulated sun orbit, controllable in Dev Tools (F3) under Sky / Day-Night.");

    ui.add_space(8.0);
    ui.strong("Time of day");
    ui.label("Runs from 0.0 (midnight) to 1.0 (next midnight): 0.25 sunrise, 0.5 noon, 0.75 sunset. Starts at noon (0.5) by default; scrub or pause it in F3.");

    ui.add_space(8.0);
    ui.strong("Sun, sky, and fog");
    ui.label("The sky and sun light transition through night blue, dawn orange, midday white, and sunset red. When sky tinting is enabled (F3 > Sky) fog blends with the sky colour, so distant terrain picks up the same hue.");

    ui.add_space(8.0);
    ui.strong("Stars");
    ui.label("Appear at night and fade at sunrise, each twinkling independently. Seeded consistently, not real constellations.");

    ui.add_space(8.0);
    ui.strong("Aircraft lights");
    ui.label("Nav (red left wingtip, green right, white tail) and strobe lights default on but can be switched off from the instrument panel. The beacon (red belly light) pulses automatically while the engine is running. Landing light toggles with N. All positions and intensities are adjustable in F3 > Lights.");

    ui.add_space(8.0);
    ui.strong("Sun orbit inclination");
    ui.label("Tiltable in F3 > Sky. At 0 the sun rises due east and sets due west; a positive inclination arcs the path to one side, giving a sense of latitude.");
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
    ui.label("Airports sit on a sparse ~6 km grid. Each cell hashes its position against the seed to decide if an airport exists, what type, its runway heading, and its name. Terrain is flattened locally so the strip sits flush on the ground.");

    ui.add_space(6.0);
    ui.strong("Ground contact");
    ui.label("The terrain mesh itself is purely visual — there are no physics colliders on it. Landing gear and hull-strike detection both sample the same height function the terrain is drawn from directly, so ground contact is exact regardless of visual LOD.");
}

// ── Tab: Multiplayer ──────────────────────────────────────────────────────────

fn tab_multiplayer(ui: &mut egui::Ui) {
    ui.label("Open the Multiplayer window from the menu bar. You're connected to the default server automatically on launch — no setup needed to see other players.");

    ui.add_space(8.0);
    ui.strong("Browse");
    ui.label("Set your display name here (a random one is picked for you at first). Below it, the server list refreshes automatically every few seconds and shows each server's player count and world seed. The official server is marked [Official]. Click Connect on any row to switch.");

    ui.add_space(8.0);
    ui.strong("Host");
    ui.label("Create your own server with a custom seed and name, defaulting to \"{your name}'s Server\". Creating one connects you to it automatically.");

    ui.add_space(8.0);
    ui.strong("Players");
    ui.label("Lists everyone else connected to your current server. Teleport to jumps your aircraft to just behind and above theirs, matching their speed and heading.");

    ui.add_space(8.0);
    ui.strong("Settings");
    ui.label("Advanced: point the client at a different master server if you don't want to use the official one.");

    ui.add_space(8.0);
    ui.strong("Notes");
    ui.label("Movement is client-authoritative — your own aircraft is simulated locally and just broadcasts its position. Switching or creating a server resets your aircraft to that world's runway.");
}
