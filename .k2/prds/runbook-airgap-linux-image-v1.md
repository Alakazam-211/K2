# Runbook — Air-gap Linux / MasterControl image (v1)

**PRD:** `.k2/prds/prd-air-gap-and-lan-listen-v1.md`  
**Status:** in tree (env-before-first-start). There is **no** `k2 daemon install --airgap` flag yet — that is a later convenience.

Goal: the daemon is air-gapped **from process start**, not “install then flip a flag.” Default is off; if the daemon starts without the env, you already had a window (updater / leftover cert).

## 1. Do not run `k2 daemon install` on the air-gapped box

That command fetches the binary from GitHub. Bake or `scp` `k2-daemon` + `cli/k2` into the image.

## 2. Before first start: systemd env

Unit or drop-in, **before first `ExecStart`**:

```
Environment=K2_AIRGAP=1
Environment=K2_LISTEN=lan
```

(`K2_LISTEN=lan` only if a second machine on the VPC should Add Server. Skip it for daemon-only.)

## 3. Pre-write the sticky port

```
mkdir -p ~/.k2
echo -n 60710 > ~/.k2/daemon.port
```

If the port is taken at boot, the daemon falls back to a new ephemeral and saved LAN Add Server URLs go stale (boot log warns).

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

## 8. Add Server from the laptop

Settings → Add Server:

```
http://<LAN-IP>:<daemon.port>
```

Example: `http://192.168.1.50:60710` + the seeded username/password.

**Not** `https://`. **Not** `:443`. LAN v1 is cleartext HTTP on the sticky daemon port.

## 9. Tuesday claim (honest)

Packet-capture: no SYN to `connect.k2.dev`, `cert.k2.dev`, Supabase, k2e-01 (`178.156.232.105`), or GitHub.

The binary still **contains** those strings. Do not say “authoritatively impossible” until a compile-time strip (out of v1). `--airgap` as an installer flag is later; this env-before-start flow is the product.

## 10. Live 3-minute probe (macOS launchd / z3mbpZ)

`tests/airgap/run-z3mbpz-airgap-lan-3min.sh` — env-only `K2_AIRGAP=1` + `K2_LISTEN=lan` on the **existing** launchd daemon, assert plane-dark + LAN `/boot-status`, restore before the 3-minute Connect lease TTL. Does not persist settings, does not unpair, does not delete `tunnel.json`. Requires `NEW_DAEMON=` pointing at an air-gap-capable `k2-daemon`.

```
NEW_DAEMON=/path/to/k2-daemon ./tests/airgap/run-z3mbpz-airgap-lan-3min.sh
```

## 11. Pointers

- Product SSOT: `.k2/prds/prd-air-gap-and-lan-listen-v1.md`
- Review (patches applied): `.k2/prds/prd-air-gap-and-lan-listen-v1-review.md`
- Wiki facts: `Feature - Air-Gap When Tunnel Off`
