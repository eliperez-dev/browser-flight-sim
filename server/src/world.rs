//! World registry: every live game world (the always-on default plus any
//! player-created ones) is a `SharedWorld`, spawned in-process and looked up
//! by `ServerId`. There is no per-world OS process or port — everything is
//! multiplexed over the one HTTP/WS listener in `main.rs`, routed by the
//! `ServerId` in the request path.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicU32},
    time::{Duration, Instant},
};

use protocol::{PlayerId, PlayerState, ServerToClient};
use tokio::sync::{Mutex, RwLock, broadcast};

pub type ServerId = String;

/// The always-on world every client defaults to. Exempt from idle reaping.
pub const DEFAULT_SERVER_ID: &str = "default";

pub struct SharedWorld {
    pub id: ServerId,
    pub name: String,
    pub seed: u32,
    pub players: Mutex<HashMap<PlayerId, PlayerState>>,
    pub next_id: AtomicU32,
    pub broadcast_tx: broadcast::Sender<(PlayerId, ServerToClient)>,
    /// Set to `None` whenever a player is connected, and to `Some(Instant)`
    /// the moment the world becomes empty. The reaper drops worlds that have
    /// sat empty longer than `IDLE_REAP_AFTER`. Always `None` for the
    /// default world (see `is_default`).
    empty_since: Mutex<Option<Instant>>,
}

impl SharedWorld {
    fn new(id: ServerId, name: String, seed: u32) -> Arc<Self> {
        let (broadcast_tx, _rx) = broadcast::channel(1024);
        Arc::new(Self {
            id,
            name,
            seed,
            players: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            broadcast_tx,
            empty_since: Mutex::new(None),
        })
    }

    pub fn is_default(&self) -> bool {
        self.id == DEFAULT_SERVER_ID
    }

    /// Called after a player joins or leaves to keep the idle clock honest.
    async fn refresh_idle_clock(&self) {
        if self.is_default() {
            return;
        }
        let empty = self.players.lock().await.is_empty();
        let mut empty_since = self.empty_since.lock().await;
        match (empty, *empty_since) {
            (true, None) => *empty_since = Some(Instant::now()),
            (false, Some(_)) => *empty_since = None,
            _ => {}
        }
    }

    async fn idle_duration(&self) -> Option<Duration> {
        self.empty_since.lock().await.map(|since| since.elapsed())
    }
}

/// How long a player-created world may sit with zero connected players
/// before the reaper drops it.
const IDLE_REAP_AFTER: Duration = Duration::from_secs(5 * 60);

/// How often the reaper sweeps the registry for idle worlds.
const REAP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Registry {
    worlds: RwLock<HashMap<ServerId, Arc<SharedWorld>>>,
}

impl Registry {
    pub async fn insert_default(&self, seed: u32) -> Arc<SharedWorld> {
        let world = SharedWorld::new(DEFAULT_SERVER_ID.to_string(), "Official Server".to_string(), seed);
        self.worlds.write().await.insert(world.id.clone(), world.clone());
        world
    }

    /// Creates a fresh in-process world with a random id and inserts it.
    /// Returns the new world so the caller can report its id back to the
    /// requesting client.
    pub async fn create(&self, seed: u32, name: String) -> Arc<SharedWorld> {
        let id = new_server_id();
        let world = SharedWorld::new(id, name, seed);
        self.worlds.write().await.insert(world.id.clone(), world.clone());
        tracing::info!("created server '{}' (id={}, seed={})", world.name, world.id, world.seed);
        world
    }

    pub async fn get(&self, id: &str) -> Option<Arc<SharedWorld>> {
        self.worlds.read().await.get(id).cloned()
    }

    /// Called by connection handlers after a player joins/leaves, so the
    /// idle clock for reaping is accurate without a dedicated poll of
    /// player counts.
    pub async fn notify_membership_changed(&self, world: &SharedWorld) {
        world.refresh_idle_clock().await;
    }

    pub async fn directory(&self) -> Vec<DirectoryEntry> {
        let worlds = self.worlds.read().await;
        let mut entries = Vec::with_capacity(worlds.len());
        for world in worlds.values() {
            let player_count = world.players.lock().await.len();
            entries.push(DirectoryEntry {
                id: world.id.clone(),
                name: world.name.clone(),
                seed: world.seed,
                player_count,
                is_default: world.is_default(),
            });
        }
        entries.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
        entries
    }

    /// Background loop: periodically drops player-created worlds that have
    /// been empty for longer than `IDLE_REAP_AFTER`. Never touches the
    /// default world.
    pub async fn run_reaper(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(REAP_INTERVAL);
        loop {
            ticker.tick().await;
            let mut to_remove = Vec::new();
            {
                let worlds = self.worlds.read().await;
                for world in worlds.values() {
                    if world.is_default() {
                        continue;
                    }
                    if let Some(idle) = world.idle_duration().await {
                        if idle >= IDLE_REAP_AFTER {
                            to_remove.push(world.id.clone());
                        }
                    }
                }
            }
            if to_remove.is_empty() {
                continue;
            }
            let mut worlds = self.worlds.write().await;
            for id in to_remove {
                tracing::info!("reaping idle server '{id}'");
                worlds.remove(&id);
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DirectoryEntry {
    pub id: ServerId,
    pub name: String,
    pub seed: u32,
    pub player_count: usize,
    pub is_default: bool,
}

fn new_server_id() -> ServerId {
    use rand::Rng;
    let n: u64 = rand::thread_rng().r#gen();
    format!("{n:016x}")
}
