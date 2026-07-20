//! Multiplayer window — toggled from the menu bar. Tabbed: Browse (server
//! directory from the configured master), Host (create a player server with
//! a custom seed), Players (list of currently connected remote players, with
//! teleport-to-player), and Settings (display name, advanced master-server
//! override).

use avian3d::prelude::{AngularVelocity, LinearVelocity};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::network::{ConnectionAction, LocalPlayerName, MasterServer, NetworkStatus, PendingConnectionAction, RemotePlayer, RemoteTarget};
use crate::network_directory::{CreateServerRequestQueue, CreateServerState, DirectoryRefreshRequest, DirectoryState};
use crate::physics::aircraft_physics::AircraftRoot;
use crate::plane::Airplane;
use crate::ui::menu_bar::MenuBar;

const BORDER: egui::Color32 = egui::Color32::from_rgb(45, 74, 122);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(139, 154, 181);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(59, 130, 246);
const GOOD: egui::Color32 = egui::Color32::from_rgb(90, 200, 130);
const WARN: egui::Color32 = egui::Color32::from_rgb(235, 90, 90);

pub struct MultiplayerMenuPlugin;

impl Plugin for MultiplayerMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MultiplayerMenuTab>()
            .init_resource::<HostServerSeed>()
            .init_resource::<HostServerName>()
            .init_resource::<DirectoryAutoRefresh>()
            .add_systems(Update, auto_refresh_directory)
            .add_systems(EguiPrimaryContextPass, draw_multiplayer_menu.in_set(crate::ui::UiSet));
    }
}

/// How often the Browse tab's server list refreshes itself while the
/// Multiplayer window is open, so player counts stay roughly live without
/// the user having to hit Refresh.
const AUTO_REFRESH_INTERVAL: f32 = 5.0;

#[derive(Resource, Default)]
struct DirectoryAutoRefresh {
    timer: f32,
    /// Tracks the window's open state across frames so we can detect the
    /// closed→open edge and refresh immediately instead of waiting a full
    /// interval the first time it's opened.
    was_open: bool,
}

/// Keeps the Browse tab's directory listing fresh: fires an immediate
/// refresh the moment the Multiplayer window opens, then re-fires every
/// `AUTO_REFRESH_INTERVAL` seconds for as long as it stays open.
fn auto_refresh_directory(
    time: Res<Time>,
    bar: Res<MenuBar>,
    mut auto: ResMut<DirectoryAutoRefresh>,
    mut dir_refresh: ResMut<DirectoryRefreshRequest>,
) {
    if !bar.multiplayer {
        auto.was_open = false;
        auto.timer = 0.0;
        return;
    }

    if !auto.was_open {
        auto.was_open = true;
        auto.timer = 0.0;
        dir_refresh.0 = true;
        return;
    }

    auto.timer += time.delta_secs();
    if auto.timer >= AUTO_REFRESH_INTERVAL {
        auto.timer = 0.0;
        dir_refresh.0 = true;
    }
}

#[derive(Resource, Default, PartialEq, Eq)]
enum MultiplayerMenuTab {
    #[default]
    Browse,
    Host,
    Players,
    Settings,
}

/// Seed field for the Host tab; kept across frames/tab switches.
#[derive(Resource)]
struct HostServerSeed(u32);

impl Default for HostServerSeed {
    fn default() -> Self {
        // A fixed default (was 1) meant everyone's first hosted server used
        // the same terrain unless they thought to change it — randomize so
        // each new server starts out as a genuinely different world.
        Self(rand::random_range(0..u32::MAX))
    }
}

/// Server-name field for the Host tab. `None` means "not yet customized by
/// the user" — the tab displays `{LocalPlayerName}'s Server` as a live
/// placeholder in that state so it stays in sync if the player edits their
/// name afterward; typing in the field switches it to `Some` and it stops
/// tracking the name.
#[derive(Resource, Default)]
struct HostServerName(Option<String>);

#[allow(clippy::too_many_arguments)]
fn draw_multiplayer_menu(
    mut bar: ResMut<MenuBar>,
    mut tab: ResMut<MultiplayerMenuTab>,
    mut contexts: EguiContexts,
    mut master: ResMut<MasterServer>,
    mut name: ResMut<LocalPlayerName>,
    status: Res<NetworkStatus>,
    mut pending: ResMut<PendingConnectionAction>,
    mut dir_state: ResMut<DirectoryState>,
    mut dir_refresh: ResMut<DirectoryRefreshRequest>,
    mut host_seed: ResMut<HostServerSeed>,
    mut host_name: ResMut<HostServerName>,
    mut create_state: ResMut<CreateServerState>,
    mut create_queue: ResMut<CreateServerRequestQueue>,
    remotes: Query<(&RemotePlayer, &RemoteTarget)>,
    mut local: Query<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), (With<Airplane>, With<AircraftRoot>)>,
) -> Result {
    if !bar.multiplayer {
        return Ok(());
    }

    // Auto-connect the newly-created server once it comes back, so "Create"
    // feels like one action instead of create-then-manually-connect.
    if let Some(Ok(id)) = create_state.result.take() {
        pending.0 = Some(ConnectionAction::Connect(format!("{}/ws/{id}", master.ws_url)));
        tab.set_if_neq(MultiplayerMenuTab::Browse);
    }

    let ctx = contexts.ctx_mut()?;

    egui::Window::new("Multiplayer")
        .open(&mut bar.multiplayer)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(120.0, 48.0))
        .default_width(340.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut *tab, MultiplayerMenuTab::Browse, "Browse");
                ui.selectable_value(&mut *tab, MultiplayerMenuTab::Host, "Host");
                ui.selectable_value(&mut *tab, MultiplayerMenuTab::Players, "Players");
                ui.selectable_value(&mut *tab, MultiplayerMenuTab::Settings, "Settings");
            });
            ui.separator();

            connection_status_line(ui, &status, &mut pending);
            ui.add_space(4.0);

            match *tab {
                MultiplayerMenuTab::Browse => draw_browse_tab(ui, &mut name, &master, &mut dir_state, &mut dir_refresh, &mut pending, &status),
                MultiplayerMenuTab::Host => draw_host_tab(ui, &mut host_seed, &mut host_name, &name, &create_state, &mut create_queue),
                MultiplayerMenuTab::Players => draw_players_tab(ui, &remotes, &mut local),
                MultiplayerMenuTab::Settings => draw_settings_tab(ui, &mut master),
            }
        });

    Ok(())
}

fn connection_status_line(ui: &mut egui::Ui, status: &NetworkStatus, pending: &mut PendingConnectionAction) {
    let (color, text) = if status.connected {
        (GOOD, "Connected")
    } else if status.server_url.is_some() {
        (egui::Color32::from_rgb(230, 200, 80), "Connecting...")
    } else {
        (WARN, "Disconnected")
    };

    ui.horizontal(|ui| {
        status_dot(ui, color);
        ui.colored_label(color, text);
        if status.connected {
            if let Some(url) = &status.server_url {
                ui.label(egui::RichText::new(url).color(TEXT_DIM).small());
            }
        }
        if status.server_url.is_some() && ui.button("Disconnect").clicked() {
            pending.0 = Some(ConnectionAction::Disconnect);
        }
    });
}

/// Small filled circle drawn with the painter rather than a Unicode glyph
/// (e.g. "●") — egui's default font doesn't ship that codepoint, so text
/// glyphs like that render as invisible/missing tofu instead of a dot.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let size = egui::vec2(8.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn draw_browse_tab(
    ui: &mut egui::Ui,
    name: &mut LocalPlayerName,
    master: &MasterServer,
    dir_state: &mut DirectoryState,
    dir_refresh: &mut DirectoryRefreshRequest,
    pending: &mut PendingConnectionAction,
    status: &NetworkStatus,
) {
    ui.horizontal(|ui| {
        ui.label("Display name");
        ui.text_edit_singleline(&mut name.0);
    });
    ui.label(egui::RichText::new("Sent to the server when you join").color(TEXT_DIM).small());
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("Servers");
        if ui.add_enabled(!dir_state.loading, egui::Button::new("Refresh")).clicked() {
            dir_refresh.0 = true;
        }
        if dir_state.loading {
            ui.spinner();
        }
    });

    if let Some(err) = &dir_state.last_error {
        ui.colored_label(WARN, format!("Directory error: {err}"));
    }

    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, BORDER))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_min_width(310.0);
            match &dir_state.entries {
                None => {
                    ui.label(egui::RichText::new("Not loaded yet").color(TEXT_DIM));
                }
                Some(entries) if entries.is_empty() => {
                    ui.label(egui::RichText::new("No servers found").color(TEXT_DIM));
                }
                Some(entries) => {
                    for entry in entries {
                        ui.horizontal(|ui| {
                            let label = if entry.is_default {
                                format!("[Official] {}", entry.name)
                            } else {
                                entry.name.clone()
                            };
                            ui.label(label);
                            ui.label(egui::RichText::new(format!("{} online", entry.player_count)).color(TEXT_DIM).small());
                            ui.label(egui::RichText::new(format!("seed {}", entry.seed)).color(TEXT_DIM).small());

                            let url = format!("{}/ws/{}", master.ws_url, entry.id);
                            let is_current = status.server_url.as_deref() == Some(url.as_str());
                            ui.add_enabled_ui(!is_current, |ui| {
                                if ui.button(if is_current { "Connected" } else { "Connect" }).clicked() {
                                    pending.0 = Some(ConnectionAction::Connect(url));
                                }
                            });
                        });
                    }
                }
            }
        });
}

fn draw_host_tab(
    ui: &mut egui::Ui,
    host_seed: &mut HostServerSeed,
    host_name: &mut HostServerName,
    name: &LocalPlayerName,
    create_state: &CreateServerState,
    create_queue: &mut CreateServerRequestQueue,
) {
    ui.label("Create a new server on the master, seeded for your own world.");
    ui.add_space(4.0);

    let default_name = format!("{}'s Server", name.0);
    ui.horizontal(|ui| {
        ui.label("Server name");
        let mut buf = host_name.0.clone().unwrap_or_else(|| default_name.clone());
        if ui.text_edit_singleline(&mut buf).changed() {
            host_name.0 = Some(buf);
        }
    });

    ui.horizontal(|ui| {
        ui.label("Seed");
        ui.add(egui::DragValue::new(&mut host_seed.0).range(0..=u32::MAX));
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(!create_state.loading, egui::Button::new("Create Server")).clicked() {
            let server_name = host_name.0.clone().unwrap_or(default_name);
            create_queue.0 = Some((host_seed.0, server_name));
        }
        if create_state.loading {
            ui.spinner();
            ui.label(egui::RichText::new("Creating...").color(TEXT_DIM));
        }
    });

    if let Some(Err(err)) = &create_state.result {
        ui.colored_label(WARN, format!("Failed to create server: {err}"));
    }
}

fn draw_players_tab(
    ui: &mut egui::Ui,
    remotes: &Query<(&RemotePlayer, &RemoteTarget)>,
    local: &mut Query<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), (With<Airplane>, With<AircraftRoot>)>,
) {
    if remotes.is_empty() {
        ui.label(egui::RichText::new("No other players connected").color(TEXT_DIM));
        return;
    }

    let mut teleport_target: Option<(Vec3, Quat, Vec3)> = None;

    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, BORDER))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            for (player, target) in remotes {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&player.name).color(ACCENT));
                    if ui.button("Teleport to").clicked() {
                        teleport_target = Some((target.position, target.rotation, target.velocity));
                    }
                });
            }
        });

    if let Some((position, rotation, velocity)) = teleport_target {
        if let Ok((mut transform, mut lin_vel, mut ang_vel)) = local.single_mut() {
            // Offset slightly behind/above the target so we don't spawn
            // stacked exactly inside their aircraft.
            transform.translation = position + Vec3::new(0.0, 5.0, 10.0);
            transform.rotation = rotation;
            lin_vel.0 = velocity;
            ang_vel.0 = Vec3::ZERO;
        }
    }
}

fn draw_settings_tab(ui: &mut egui::Ui, master: &mut MasterServer) {
    ui.collapsing("Advanced", |ui| {
        ui.label(egui::RichText::new("Master server (directory + hosting)").color(TEXT_DIM));
        ui.horizontal(|ui| {
            ui.label("HTTP");
            ui.text_edit_singleline(&mut master.http_url);
        });
        ui.horizontal(|ui| {
            ui.label("WS");
            ui.text_edit_singleline(&mut master.ws_url);
        });
        ui.label(
            egui::RichText::new("Point this at a self-hosted master to leave the official servers entirely.")
                .color(TEXT_DIM)
                .small(),
        );
    });
}
