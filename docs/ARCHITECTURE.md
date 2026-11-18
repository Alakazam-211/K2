# K2 Architecture

A one-page map. K2 is daemon-first: one long-running daemon (`crates/k2-daemon`, built on the library `crates/k2-core`) owns the data, the terminals and the agents, and keeps working with the app closed. Everything a person or an agent touches is a front-end of that daemon: the desktop app, the `k2` CLI, remote clients through K2 Connect, and the phone app. The front-ends hold no logic of their own. They ask the daemon and show the answer.

## The shape

```
  Desktop app                  k2 CLI           Remote clients         Phone app
  src-tauri/ (Tauri shell)     cli/k2           through K2 Connect     (source not
  src/renderer/ (React)                         (tunnel and relay)      in this repo)
        |                         |                    |                    |
        +-------------------------+--------------------+--------------------+
                                  |
                HTTP /cli/* and /v1/*, WebSocket streams,
                /boot-status checked first
                                  |
                +-----------------------------------+
                |             k2-daemon             |
                |   routes, policy, live streams    |
                |   k2-core: db, terminals,         |
                |   scheduler, agents, tunnel       |
                +-----------------------------------+
                  |        |        |         |           |
               SQLite    vault    k2-ptyd   k2-root-    compute nodes
               (k2-core  (k2-     (holds    helper      (k2-node, they
                db)      vault)   PTYs)     (Linux)     dial in)
```

## How clients talk to the daemon

- **HTTP routes.** Almost everything is a route under `/cli/*`. One function matches them all: `crates/k2-daemon/src/routes/dispatcher.rs`. Each `/cli/*` route has a row in the route policy table (`ROUTES` in `crates/k2-daemon/src/routes/route_policy.rs`, rows in `crates/k2-daemon/src/routes/policy/`) that says the lowest login role it admits. A smaller `/v1/*` set, matched in the same dispatcher, is the door for API keys.
- **WebSocket streams.** Live data is pushed, not polled. The main ones:

  | Stream | File in `crates/k2-daemon/src/` |
  |---|---|
  | `/events`, agent hook events | `events.rs` |
  | `/cli/sessions/events`, session lifecycle per workspace | `session_events_ws.rs` |
  | `/cli/sessions/subscribe`, one session's frames | `sessions_ws.rs` |
  | `/cli/sessions/bytes`, one session's raw bytes | `sessions_bytes_ws.rs` |
  | `/cli/sessions/grid`, terminal grid snapshots and deltas | `sessions_grid_ws.rs` |
  | `/cli/awareness/subscribe`, the awareness bus | `awareness_ws.rs` |
  | `/cli/overlay/events`, Thread and overlay pushes | `overlay_ws.rs` |
  | `/cli/activity/events`, agents at work in one workspace | `activity_events_ws.rs` |
  | `/cli/fs/events`, file changes in one workspace | `fs_events_ws.rs` |
  | `/cli/ops/stream`, session and awareness events on one socket, read only | `ops_stream_ws.rs` |

- **`/boot-status` comes first.** The daemon binds its port before it runs migrations, and the dispatcher answers `503` to every route except `/ping`, `/health`, `/boot-status` and the two public key routes until the boot is done (`crates/k2-daemon/src/boot_status.rs`, `is_ready`). The body gives a `phase` (`starting`, `migrating`, `ready` or `error`), the `protocol` range, and for a signed-in or local caller the `version` and `features`. That full view also carries `rootHelper` for fleet monitoring: `{state, protocol, version}` on Linux (`state` is `root`, `missing`, `not_allowed`, `legacy`, `package`, `rolled_back`, `airgapped`, or `unknown` before the first probe), read from the daemon's last helper probe and auto-heal, never a root call per request; `null` on macOS and Windows. The desktop app mounts only against a local daemon of its own version in phase `ready` (`src/renderer/components/ConnectionGate.tsx`). The CLI polls the same route after a restart (`cli/src/daemon.sh`).
- **Finding the local daemon.** At start the daemon writes `daemon.port` and `daemon.token` into the K2 home, `~/.k2` by default (`crates/k2-daemon/src/daemon_main.rs`). The port is kept across restarts when it can be (`crates/k2-core/src/port_claim.rs`). The token is new on every boot. The desktop shell reads both files in `src-tauri/src/daemon_client.rs`, the CLI in `cli/src/daemon.sh`.
- **K2 Connect.** The daemon runs the client side of the tunnel that makes it reachable from outside: `crates/k2-core/src/tunnel/`. The relay server is not in this repo. Remote clients then use the same routes and streams as local ones.
- **Phone.** The phone app's source is not in this repo. What is here is the daemon side: a phone signs in with a device session it gets by redeeming a one-time pairing code (`crates/k2-core/src/connect_device_pairing.rs`, `crates/k2-daemon/src/connect_device_routes.rs`).

## What the daemon owns

- **SQLite.** The database code is `crates/k2-core/src/db/`. The schema is the numbered migrations in `crates/k2-core/drizzle_sql/`.
- **The vault.** `crates/k2-vault`, linked into the daemon. The daemon's side is `crates/k2-daemon/src/vault/`.
- **k2-ptyd.** A separate process that holds terminal sessions. After a restart the daemon takes them back (`crates/k2-daemon/src/ptyd_adopt.rs`, `crates/k2-core/src/terminal/ptyd/`).
- **The root helper.** K2's one root door on Linux. The daemon's side is `crates/k2-daemon/src/root_helper/`.
- **Compute nodes.** A node connects to the daemon over the `/cli/compute/attach` WebSocket (`crates/k2-daemon/src/compute_ws.rs`) and runs the jobs it is given.

## Where things live

| Part | Path | What it is |
|---|---|---|
| Daemon | `crates/k2-daemon` | The long-running process: routes, policy, streams, scheduler and agent orchestration. |
| Core library | `crates/k2-core` | Device-local logic the daemon links: db, terminal, scheduler, agent hooks, push. |
| Thin-client helpers | `crates/k2-client` | What a thin client needs without k2-core: paths, process bootstrap, account session. |
| Vault | `crates/k2-vault` | One encrypted store for passwords, tokens and keys, used in-process by the daemon. |
| Terminals | `crates/k2-ptyd`, `crates/k2-core/src/terminal/` | A small per-user PTY host, so sessions survive a daemon restart, and the daemon's terminal code. |
| Compute | `crates/k2-node`, `crates/k2-node-proto` | The compute node runtime, and the wire between it and a controller daemon. |
| Root helper | `crates/k2-root-helper`, `crates/k2-root-proto` | K2's one root door on Linux, and its protocol. |
| Apps | `crates/k2-app-engine` | Drives rootless Podman for guarded K2 Apps, on the daemon's orders only. |
| Desktop shell | `src-tauri/` | The Tauri app: windows, menu, tray, and the connection to the daemon. |
| Renderer | `src/renderer/` | The React front-end, a thin client of the daemon. |
| CLI | `cli/src/`, `cli/text/`, `cli/k2` | Shell parts, joined into the one file `cli/k2` by `scripts/build-cli.sh`, with the printed text in `cli/text/` (the `k2 study` pages and the static help) inlined at each `%include` line. Edit a part or a text file, never `cli/k2`. |
| SDK | `sdk/` | The widget frame runtime (`sdk/k2-runtime.js`), the server SDK and generated contract files. |
| Migrations | `crates/k2-core/drizzle_sql/` | Numbered SQL migrations for the SQLite database. |
| Contract catalog | `crates/k2-core/src/contract/catalog.json` | The verb catalog: a verb layer over the route table, not a second one. |
| Tests | `tests/README.md` | The map of every place tests live and how to run them. |

The workspace `Cargo.toml` lists every crate. The "Project Structure" part of [CONTRIBUTING.md](../CONTRIBUTING.md) says in one line what each is for.

## Rules that follow from the design

- **Logic goes in the daemon or the core.** `crates/k2-daemon` and `crates/k2-core` are the only places for behaviour. That is what lets the CLI, the desktop app and a remote client all get the same answer.
- **`src-tauri/` is connection and OS work only.** It links `crates/k2-client`, not k2-core (see the note in `src-tauri/Cargo.toml`; k2-core is there for tests and benches only). A Tauri command is for a window, the menu, the tray or the link to the daemon.
- **The renderer renders what the daemon answers.** It calls routes through `src/renderer/lib/daemon-cli.ts` and listens to the streams. It does not decide.
- **A new feature is a route plus a policy row.** Then a CLI verb and a renderer call on top. The steps are in [CONTRIBUTING.md](../CONTRIBUTING.md), "How to Add a Feature or Command".
