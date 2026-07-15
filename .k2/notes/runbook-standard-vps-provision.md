# Runbook — Standard K2 Server on a Plain VPS (sandboxes OFF)

The long-pending "sandboxes-OFF shared-VPS runbook" (follow-up from the
0.40.21 sandbox arc). It is now **runbook-as-code**:
`scripts/provision-k2-server.sh` does everything below in one shot; this
document explains it, covers the manual/Pi path, and holds the
acceptance checklist for K2 Cloud Phase 0
(`.k2/prds/prd-k2-cloud-hosted-servers-v1.md`).

Scope: shared-vCPU VPS (Hetzner CX/CPX, any cloud), home server, or
Raspberry Pi 4/5 (64-bit OS). **No `/dev/kvm` required — sandboxing is
deliberately OFF.** For the sandbox-capable Dedicated bootstrap see
`.k2/notes/runbook-self-host-sandbox-server.md`.

## One-shot provision

```sh
# as root on Ubuntu 22.04/24.04, Debian 12, or Raspberry Pi OS 64-bit
curl -fsSL https://raw.githubusercontent.com/Alakazam-211/K2/main/scripts/provision-k2-server.sh -o provision-k2-server.sh
chmod +x provision-k2-server.sh

K2_TUNNEL_TOKEN=k2c_...           # from your k2.dev subdomain (dashboard → RLS row)
K2_SUBDOMAIN=alice \
K2_OWNER_USER=alice \
./provision-k2-server.sh
```

Idempotent — re-run to converge. `--bake` runs only the image-prep steps
(used by `scripts/build-k2-golden-image.sh` for the K2 Cloud snapshot).
Cloud-init automation: `scripts/cloud-init-k2-server.yaml.tpl`.

## What it does (and why)

1. Packages: curl, **minisign** (mandatory binary verification), python3,
   openssl, git, ca-certificates.
2. Service user `k2` (daemon must be NON-root; sandbox arc lesson —
   claude refuses `--dangerously-skip-permissions` as root, and root is
   the wrong posture anyway).
3. **frpc v0.61.1** pinned to the relay's frps (`k2-connect/infra/install.sh`
   pins the same). NOT auto-downloaded by the daemon — must be on PATH.
4. Daemon via `scripts/install-daemon.sh` (minisign + sha256 verified
   against `daemon-latest.json`), installed to `/home/k2/.local/bin`,
   with `K2_NO_SERVICE=1` — we supervise with a SYSTEM unit instead.
5. `k2` CLI → `/usr/local/bin/k2` (bash script; needs curl/python3/openssl).
   **⚠ MUST be `chown`ed to the DAEMON USER** (provision script does this
   as of 2026-07-15). The daemon self-stages the embedded CLI at every
   boot (`cli_stage`, 0.40.41+) so updates carry the CLI forward — a
   root-owned file makes that a silent per-boot `Permission denied` WARN
   and the CLI freezes at provision-day version while the daemon keeps
   updating (bit nsi at 0.40.41 + rpmavs; found 2026-07-15). Verify after
   any provision/migration: `ls -la /usr/local/bin/k2` owner == daemon
   user, and `grep K2_CLI_VERSION /usr/local/bin/k2` == daemon version.
   (0.40.50+ daemon also falls back to `~/.local/bin/k2` on EACCES.)
6. `~/.k2/tunnel.json` (0600, owner k2): `{token, subdomain,
   auto_start: true}` — written BEFORE first start so boot auto-dials;
   `server_addr`/`server_port` ride the compiled defaults
   (`crates/k2-core/src/tunnel/config.rs`), `local_port` self-fills.
7. `/etc/systemd/system/k2-daemon.service`: `User=k2`, `Restart=always`,
   **NO `K2_SANDBOX*` env** — this is what makes it a Standard host
   (sandbox 409s by design; `can_sandbox()` false).
8. Waits for `~/.k2/daemon.{port,token}` → creates the first owner user
   (`POST /cli/users/add` + `set-role owner` with the on-box daemon
   token). Password generated if not provided (printed once, or sent to
   the control-plane callback and never logged).
9. Optional callback POST → K2 Cloud control plane flips
   `servers.status = online`.

## Acceptance checklist (Phase 0)

- [x] Fresh hcloud CPX VM + cloud-init user_data → `k2-daemon` active,
      0.40.28 ready, owner created — with ZERO interactive SSH.
      **PASSED 2026-07-06** (k2-accept-01, ~45s on RAW ubuntu-24.04,
      self-fetch path — golden image only makes it faster).
- [x] Golden image: bake → snapshot → create-from-snapshot + personalize
      cloud-init → **ONLINE IN ~10s** (k2-accept-02, snapshot
      `k2-golden-standard-0.40.28-20260706` id 405546816). **PASSED
      2026-07-06.** Bake gotchas fixed: bake VM needs an --ssh-key (else
      Hetzner bakes an EXPIRED root password into the image, locking out
      every descendant) + `passwd -x -1 root` + `cloud-init clean` before
      poweroff. Micro-optimization available: personalize re-verifies the
      daemon binary (~1 download); skip-if-version-matches would shave a
      few seconds.
- [ ] Tunnel-attach: `https://<label>.k2.dev/boot-status` = 200 + owner
      login at the portal + desktop Remote Sign-In. PENDING a test
      subdomain token (needs `K2_TUNNEL_TOKEN`/`K2_SUBDOMAIN` env).
- [x] Owner login round-trip on-box (`/cli/auth/login` → whoami
      `role:"owner"`). **PASSED 2026-07-06** (bare metal + fresh VM).
- [x] `/v1` absent (404) on a Standard host — never-unsandboxed invariant.
      **PASSED 2026-07-06.** (409-with-API-on variant needs K2_SANDBOX_API
      set — deferred to the API-completion arc's gate split.)
- [x] Linux two-version self-update e2e 0.40.27→0.40.28. **PASSED
      2026-07-06** on bare metal — after finding + fixing THREE fleet
      bugs (see gotchas: CI secrets, sig format, KillMode).
- [x] Re-run provision script → converges. **PASSED 2026-07-06**
      (3 consecutive runs; surfaced + fixed Text-file-busy and
      stale-token-on-restart bugs).
- [ ] Raspberry Pi (aarch64) smoke: same script, same checklist items
      (community/self-host path). PENDING hardware.

## Known prerequisites & gotchas

- **CI Linux artifacts**: `daemon-binaries.yml` had been failing on
  EVERY tag since ≤0.40.24 — the `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`
  Actions secrets were never set on Alakazam-211/K2 (lost in the repo
  re-home). Fixed 2026-07-06 (`gh secret set` from the local key +
  `.env` password). If Linux assets are missing on a release, check
  those secrets first.
- **daemon-latest.json may be macos-only**: `release.sh` Step 8.5 only
  merges Linux entries when CI artifacts are pre-staged in
  `target/release/daemon-dist/` at release time — and CI finishes AFTER
  the release step. Until release.sh gains a post-CI merge step, the
  manifest on a fresh tag may lack `linux-*` keys → `install-daemon.sh`
  exits "no complete artifact for platform". Workarounds: pass
  `K2_VERSION` pointing at the newest tag whose manifest has Linux
  entries, or re-upload an updated manifest to the release
  (macos entries byte-identical, linux keys added).
- **install-daemon.sh is NOT uploaded as a release asset** — its own
  header one-liner (`releases/latest/download/install-daemon.sh`) 404s.
  Fetch from raw.githubusercontent (what the provision script does), or
  add it to release.sh's asset list (follow-up).
- **Sig-asset format (fixed fleet-wide 2026-07-06)**: tauri's signer
  emits `.sig` files as BASE64 of the minisig text; the daemon
  self-updater (`minisign_verify::Signature::decode`) and plain
  `minisign` need the RAW text — Shape B self-update had NEVER worked on
  any platform. Fixed at the source (CI workflow + release.sh decode
  before upload), the v0.40.27/v0.40.28 assets re-uploaded decoded, and
  both shell installers now accept either form. A verify_minisign
  accept-both fix in the daemon is queued for the Phase 1 window.
- **`KillMode=process` is REQUIRED on systemd units** (fixed 2026-07-06):
  the Shape B swap helper is setsid-detached but stays in the daemon's
  cgroup — default control-group kill reaped it before it could swap, so
  self-update silently no-oped on Linux. Both unit templates now set it;
  existing boxes need the drop-in
  (`/etc/systemd/system/<unit>.service.d/killmode.conf`). Applied to the
  bare-metal box's main daemon 2026-07-06 (takes effect its next stop).
- Ubuntu minimal images may lack `sudo` — the provision script uses
  `sudo -u`; install sudo or swap to `runuser` if you hit that.
- The daemon binds **127.0.0.1 only**; remote access is exclusively the
  tunnel. Do not "fix" this by exposing the port.
- Hetzner API token for K2 Cloud lives at `~/.config/hcloud/k2-cloud.token`
  (0600, OUTSIDE any git tree) on the dev machine — named `k2-cloud` in
  the Hetzner console. Test/acceptance boxes: `k2-accept-01` (throwaway).
