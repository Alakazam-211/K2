# Runbook — Air-gap Linux / MasterControl image (v1)

**PRD:** `.k2/prds/prd-air-gap-and-lan-listen-v1.md`  
**Example Caddyfile:** `docs/caddy-airgap-lan.Caddyfile`  
**Status:** air-gap + LAN listen shipped **0.40.109**. This runbook is the **MasterControl air-gapped LAN-only** topology (Caddy in front) locked **0.40.110**. There is **no** `k2 daemon install --airgap` flag yet — that is a later convenience.

Goal: the daemon is air-gapped **from process start**, not “install then flip a flag.” Default is off; if the daemon starts without the env, you already had a window (updater / leftover cert).

## 0. Topology (locked)

| Piece | MasterControl (this runbook) | Lab only |
|---|---|---|
| `K2_AIRGAP` | `1` (required, before first start) | same |
| `K2_LISTEN` | **unset** — daemon stays `127.0.0.1` | `lan` — daemon binds `0.0.0.0` |
| LAN socket | **Caddy**, one high random TCP port | the daemon sticky port |
| Example ports | Caddy **38471** → daemon `127.0.0.1:60710` | Add Server `http://<LAN-IP>:60710` |
| Add Server | `http://<LAN-IP>:38471` | `http://<LAN-IP>:<daemon.port>` |
| IDS / firewall | **that Caddy port only** | the daemon port |

Do **not** also set `K2_LISTEN=lan` on the MasterControl image. That opens a second LAN door the IDS on 38471 will miss.

Do **not** put Caddy on 80, 443, 8080, or 8443. Chris’s bar is one **high random** port so all client HTTP + WebSocket is monitorable there. **38471** is the documented example — if they pick another high port, the Caddyfile and Add Server URL must match.

LAN v1 is **cleartext HTTP** unless they terminate TLS in Caddy (not required for v1). **Not** `https://`. **Not** `:443`.

## 1. Do not run `k2 daemon install` on the air-gapped box

That command fetches the cloud binary from GitHub. Do **not** take `k2-daemon-linux-<arch>` from the public GitHub release for this image.

Bake or `scp` a **custom/enterprise** daemon built with:

```
cargo build --release --bin k2-daemon --features airgap
```

`--features airgap` bakes air-gap **on** (`K2_AIRGAP=0` cannot disable it) and **omits** the GitHub `daemon-latest.json` URL, so `POST /cli/daemon/update/check` cannot ping GitHub for update availability. That binary is **not** a GitHub Release asset and is **not** in `daemon-latest.json`. **License:** not covered by the self-serve Commercial Hosting Grant — Alakazam ships it only under a **written Enterprise agreement** (`COMMERCIAL_HOSTING_GRANT.md`). Hand it to that customer off-band (scp / future enterprise portal) — not `gh release download`.

Still set `Environment=K2_AIRGAP=1` on the unit (§2) so interactive CLI on that box refuses Connect/install the same way. Other `github.com` strings may still grep (D7 full strip is out of v1). The update-availability ping URL is the one this compile removes.

## 2. Before first start: systemd env

Unit or drop-in, **before first `ExecStart`**:

```
Environment=K2_AIRGAP=1
```

**Do not** set `K2_LISTEN=lan`. The daemon stays on `127.0.0.1`. Caddy is the only LAN listener (§8).

## 3. Pre-write the sticky port

```
mkdir -p ~/.k2
echo -n 60710 > ~/.k2/daemon.port
```

If the port is taken at boot, the daemon falls back to a new ephemeral. Caddy’s `reverse_proxy` target would then be wrong — boot log warns. Keep 60710 free on loopback.

## 4. Seed a connect-user

LAN auth is a connect-user on **that** daemon, not the on-box owner token.

Consume-once (deleted after first start) — `~/.k2/seed-users.json`:

```json
[{ "username": "ops", "password": "…", "role": "admin", "mustChangePassword": true }]
```

Live format (see `crates/k2-daemon/src/seed_users.rs`): array of `{ username, password, role, mustChangePassword? }`. Roles: `owner` | `admin` | `member` | `viewer`.

Or after start, on that box (owner CLI, loopback): `k2 users add`. Without a user, Add Server has no one to log in as.

## 5. Never write leftover Connect state

Do not write a `tunnel.json` subdomain, `unpaired.json`, or federation outbox. Those are the dirty-image leaks (cert broker / unpair POST / `https://<sub>.k2.dev`).

## 6. Mail off

No mail domains. Do not start Stalwart. v1 does not gate mail OAuth/DNSBL.

## 7. Interactive CLI also needs the env

systemd env does **not** cover an SSH session. On the box:

```
export K2_AIRGAP=1
```

Otherwise `k2 connect login` / `k2 publish subdomain *` / `k2 daemon install` skip the daemon and would still curl.

## 8. Caddy = the one monitored LAN port

Chris’s requirement: **Caddy listens on a specific port** so all client traffic can be monitored there.

- Daemon: `127.0.0.1:60710` (sticky `daemon.port`) — **not** on the LAN.
- Caddy: **one** TCP port — example **38471**. Firewall/IDS that port only.
- Add Server: `http://<LAN-IP>:38471` + seeded username/password. **Not** the daemon port.

Tracked starter: `docs/caddy-airgap-lan.Caddyfile`. They own the file on the box. Bind a **host:port**, not a bare `:port`, if they do not want every interface.

```
http://192.168.1.50:38471 {
    reverse_proxy 127.0.0.1:60710
}
```

Replace `192.168.1.50` with the box’s LAN IP. A single `reverse_proxy` covers `/boot-status`, `/cli/*` (including grid + events WebSockets), `/events`, and `/v1`. Caddy upgrades WebSockets by default.

Do not also open `daemon.port` in the security group. Do not run a second proxy on 80/443.

## 9. Tuesday claim (honest)

Packet-capture: no SYN to `connect.k2.dev`, `cert.k2.dev`, Supabase, k2e-01 (`178.156.232.105`), or GitHub.

A `--features airgap` daemon omits the GitHub `daemon-latest.json` update-ping URL. Other hosted strings (`connect.k2.dev`, relay IPs) still grep — do not say “authoritatively impossible” for those until a full D7 strip. That binary is not published on GitHub Releases. `--airgap` as an installer flag is later; env-before-start plus the custom compile is the product.

## 10. Live 3-minute probe (macOS launchd / z3mbpZ)

`tests/airgap/run-z3mbpz-airgap-lan-3min.sh` — env-only `K2_AIRGAP=1` + `K2_LISTEN=lan` on the **existing** launchd daemon, assert plane-dark + LAN `/boot-status`, restore before the 3-minute Connect lease TTL. Does not persist settings, does not unpair, does not delete `tunnel.json`. Requires `NEW_DAEMON=` pointing at an air-gap-capable `k2-daemon`.

That probe is the **lab** path (`K2_LISTEN=lan`). It does **not** stand in for the MasterControl Caddy image.

```
NEW_DAEMON=/path/to/k2-daemon ./tests/airgap/run-z3mbpz-airgap-lan-3min.sh
```

## 11. Pointers

- Product SSOT: `.k2/prds/prd-air-gap-and-lan-listen-v1.md`
- Review (patches applied): `.k2/prds/prd-air-gap-and-lan-listen-v1-review.md`
- Example Caddyfile: `docs/caddy-airgap-lan.Caddyfile`
- Wiki facts: `Feature - Air-Gap When Tunnel Off`
