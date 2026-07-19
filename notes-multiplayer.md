# Multiplayer scoping

Status: idea/scoping doc, not implemented. Written 2026-07-19.

## Constraint that shapes everything

Client is wasm32-in-browser (`wasm-bindgen`, `wasm-server-runner`). Browsers can only
do real-time networking over **WebSocket** or **WebRTC** — no raw TCP/UDP sockets from
client code. WebRTC (unreliable data channels) gets you lower latency but drags in
ICE/STUN/SDP negotiation. Start with WebSocket; revisit WebRTC only if WS latency is
provably a problem later.

Terrain (`src/terrain/generator.rs`) is already fully deterministic from a single
`seed: u32` (layered Perlin noise, `PerlinLayer::new(seed + N, ...)`). That means the
server never needs to ship heightmap/mesh data — just the seed, once, at join time.
This is the biggest bandwidth win available and should stay true (don't let terrain
gen grow non-seed inputs, e.g. external data files, without also plumbing them through
the join handshake).

## Server model

One server process = one world = one seed. Not "servers are Tokio threads" — Tokio's
model is tasks, not OS threads. Per server process:

- One Tokio runtime.
- One spawned task per connected player (owns that player's WS connection, reads
  input/state updates, writes broadcasts to them).
- One authoritative tick loop task (say 20–30Hz) that holds shared world state
  (player list, positions, world time) and produces broadcast deltas. Tasks talk to
  it via `tokio::sync::mpsc` channels — never shared mutable state across tasks.

If later you want one binary to host multiple communities' worlds, that's multiple
independent instances of the above inside one process (different seeds, different
player sets), not literally "a thread per server."

### Player state authority

Client-authoritative: each client simulates its own aircraft physics locally (already
true today, single-player) and broadcasts its transform/velocity at tick rate. Server
relays to other clients, doing only sanity clamps (speed/teleport bounds) — not full
physics replay. Other players' aircraft are dead-reckoned/interpolated between updates
on each client.

This fits a casual/co-op flight sim: no competitive integrity requirement, and it
avoids running `avian3d` physics server-side or building client-side prediction +
reconciliation (which is what real anti-cheat authoritative netcode requires). If
griefing/cheating becomes a real problem later, revisit — but don't build for it
up front.

## Wire protocol (sketch)

Messages over one WebSocket per player, small serde-serialized enum (bincode or
similar — avoid JSON for the high-frequency state messages, fine for handshake).

```
ClientToServer:
  Join { name: String }
  StateUpdate { pos, rot, velocity, control_surfaces, timestamp }
  Leave

ServerToClient:
  Welcome { your_id, seed, world_time, other_players: Vec<PlayerState> }
  PlayerJoined { id, name, initial_state }
  PlayerStateUpdate { id, pos, rot, velocity, control_surfaces, timestamp }
  PlayerLeft { id }
  Kick { reason }   // e.g. sanity-check failure
```

`control_surfaces` (aileron/elevator/rudder/flap deflection) is optional-but-cheap and
makes other players' planes look right instead of just interpolated hulls sliding
around — worth including from the start since the data already exists in
`AeroSurface`/`airplane_controller`.

## Server discovery / the "multiplayer tab"

This is a separate concern from game networking — don't conflate it with the game
server itself.

- A small **master/directory server** (its own lightweight service — REST or WS) that
  custom servers register with and heartbeat to (name, seed, current player count,
  address/port). This is the thing the client's multiplayer tab actually queries to
  render the server list.
- Each custom server periodically pings the master ("alive", updated player count).
  Classic Minecraft server-list / Source master-server pattern. Keeps NAT/hosting
  complexity on your one directory service instead of every server operator needing
  DNS or a public listing mechanism themselves.
- Directory API is intentionally boring: `GET /servers -> [{name, seed, address,
  player_count, max_players}]`, `POST /register` + periodic `POST /heartbeat` from
  servers.

## Repo/crate structure

Currently a single crate, no workspace. The Tokio server binary can't be wasm32 and
shouldn't share the bevy/avian3d/wasm dependency tree, so this wants to become a
Cargo workspace:

```
Cargo.toml           # [workspace] members = ["client", "server", "protocol"]
protocol/            # shared serde types: the wire enum above, seed/world config
client/              # current src/, + a networking plugin (WS via web-sys or a wasm-compatible ws crate)
server/              # new Tokio binary: game server + (maybe separate bin for) master directory
```

`protocol` as its own tiny crate keeps client and server from drifting out of sync on
message shapes, and keeps `server`'s Tokio deps out of the wasm build entirely.

## Rough milestones

1. **Workspace split** — pull existing code into `client/`, add empty `server/` and
   `protocol/` crates, confirm wasm build still works unchanged.
2. **Protocol + single-server loopback** — define the enum above, get a Tokio server
   relaying position updates between two native (non-wasm) test clients, no directory
   yet, hardcoded address.
3. **Browser client networking** — WS connection from wasm client (web-sys WebSocket
   or a wasm-compatible crate), join handshake, render other players' aircraft as
   interpolated ghost planes.
4. **World seed sync on join** — server sends seed + world time in `Welcome`, client
   terrain gen consumes it instead of the hardcoded `seed: 3` in
   `terrain/generator.rs:155`.
5. **Master directory + multiplayer tab UI** — directory service, server
   register/heartbeat, egui multiplayer tab listing servers with name/seed/player
   count, connect-on-click.
6. **Polish** — reconnect handling, sanity-check kicks, player name tags/nameplates,
   basic chat if wanted.

Each milestone is independently testable and shippable — don't need the directory
service working to validate the core relay loop, etc.

## Open questions to settle before/while building

- Max players per server — affects whether naive O(n) broadcast-to-all is fine
  (almost certainly yes for a flight sim's likely scale, tens not hundreds).
- Does world *time* (day/night cycle) need to be server-synced too, or is each
  client's local clock fine? Cheap to sync (server sends it in `Welcome` +
  periodic corrections), avoids client drift.
- Persistent server identity — do custom servers need a stable ID so the directory
  can tell "the same server restarted" from "a new server," or is name+address enough
  for v1? Probably fine to skip for v1.
