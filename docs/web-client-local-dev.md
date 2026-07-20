# Hosted web client — local try

Run the browser SPA against a local K2 daemon on your laptop. Same same-origin
shape as production (`loader` + versioned `/app/<ver>/` + data plane proxy),
with **no Cloudflare / R2** required.

Product design: [`prd/prd-hosted-web-client-and-edge-delivery-v1.md`](../prd/prd-hosted-web-client-and-edge-delivery-v1.md)
(phases 1–2 of §7.5).

---

## Prerequisites

| Need | Notes |
|---|---|
| **Daemon running** | Local `k2-daemon` listening on loopback. Port is written to `~/.k2/heartbeat.port` (legacy fallback: `~/.k2so/heartbeat.port`). |
| **Connect user** | At least one owner/admin for RemoteSignIn: `k2 users add <name> --role owner` (prompts for password). |
| **Caddy** | Same-origin reverse proxy for the built path. macOS: `brew install caddy`. |
| **Bun** | Package scripts and Vite web build: `bun` on `PATH`. |

Optional: set `K2_DAEMON_PORT` yourself if the heartbeat file is missing or you
want an explicit port.

---

## Build the SPA

```sh
bun run vite:build:web
```

Output lands under:

```text
out/web/app/<version>/
```

`<version>` is `package.json`’s `version` (also the Vite `base` path
`/app/<version>/`). Rebuild after renderer changes.

---

## Serve (production-shaped local edge)

```sh
bun run web:serve
```

What this does (`scripts/web-serve.sh` + `web/Caddyfile`):

1. Reads the daemon port from `~/.k2/heartbeat.port` (or `K2_DAEMON_PORT`).
2. Serves the tiny loader from `web/loader/` at `/`.
3. Serves the built SPA at `/app/<version>/` from `out/web/app/<version>/`.
4. Proxies `/boot-status`, `/cli/*`, and `/events` to `127.0.0.1:<daemon-port>`.

Default listen: **http://127.0.0.1:8080/** (`K2_WEB_PORT` to change).

If Caddy is missing, the script exits with install hints — do not open the
static files on a different origin; CORS will block `/cli` and `/events`.

Smoke (no GUI): `bash scripts/web-client-smoke.sh` — requires daemon heartbeat; builds SPA if missing; asserts `/boot-status`, loader `/`, and `/app/<ver>/index.html` via a short-lived proxy (or direct checks). Optional login: `K2_WEB_SMOKE_USER` / `K2_WEB_SMOKE_PASS`.

---

## What you should see

Open **http://127.0.0.1:8080/**

1. **Loader** (`/`) — short-cached entry HTML/JS.
2. **`/boot-status`** — loader picks `webClientVersion` (or `version`).
3. **SPA** — navigates to `/app/<ver>/` (content-hashed assets).
4. **RemoteSignIn** — full-screen connect-user login against the same origin
   (daemon `connect-users` via `/cli/auth/login`). Sign in with the user you
   created via `k2 users add`.

The web build forces a single same-origin host (no “local” Tauri daemon path).

---

## Dev mode (Vite HMR)

Skip the production build/Caddy path while iterating on the SPA:

```sh
K2_DAEMON_PORT="$(tr -d '[:space:]' < ~/.k2/heartbeat.port)" bun run vite:dev:web
```

- Dev server listens on **port 5174** (`vite.config.web.ts`).
- When `K2_DAEMON_PORT` is set, Vite proxies `/boot-status`, `/cli`, and
  `/events` to that daemon (same-origin for the browser).
- Without `K2_DAEMON_PORT`, the SPA still loads but the data plane has no
  proxy target.

Open **http://127.0.0.1:5174/** (or the URL Vite prints). This path does **not**
use the edge loader; HMR serves the SPA directly with `VITE_WEB` defined.

---

## Support override: `?v=`

On the **loader** path (`web:serve` / production edge):

```text
http://127.0.0.1:8080/?v=0.40.53
```

Forces the loader to load `/app/<that-version>/` instead of the version from
`/boot-status`. Same semver validation and support floor as boot-status. Use
for support when a box advertises the wrong client version or you need a
known-good bundle.

---

## Known amputations (browser vs desktop)

The web client is remote-only and deliberately thinner than Tauri:

| Area | Web behavior |
|---|---|
| **Browser tab** | Native embedded browser pane is not available (Tauri webview-only). |
| **Updater** | App self-update / install-update is stubbed; no desktop updater. |
| **Local filesystem** | No local FS / native drag paths; work happens on the daemon box. |
| **Keychain** | OS keychain is stubbed; token storage uses `sessionStorage` / `localStorage` shims for Phase 1. |
| **Multi-host switcher** | Locked to the single same-origin host — no Local, no “Add a server…”. |

Expect full remote workspace/terminal parity for the remote data plane; do not
oversell desktop-only surfaces.

---

## Auth notes (Phase 1)

- **Token auth only** for now (query/bearer + browser session storage after
  login). **HttpOnly cookies** are a later phase (PRD §2.3 / phase 2).
- Phase 1 may still use token-in-query on some paths for parity with desktop
  remote clients — that is **not** the production web design.
- **Do not put credentials in URLs** in production design: query tokens leak
  into history, edge/access logs, and `Referer`. Cookie + CSRF header is the
  intended browser transport once phase 2 lands.

---

## No R2 / Cloudflare for local try

Phases 1–2 deliberately prove client ↔ daemon over **laptop Caddy** (or Vite
dev proxy). You do **not** need:

- Cloudflare `*.app.k2.dev` / ACM
- R2 (or other) bundle bucket
- Relay / tunnel cutover

Those arrive for CI publish + edge (PRD §7.5 phases 3–4). Local try only needs
daemon + build + Caddy (or `vite:dev:web`).

---

## Publish to R2 (CI / release)

Phase 3 publishes the production-shaped SPA to the Cloudflare R2 bucket so the
edge can serve immutable `app/<ver>/` bundles (PRD §3 / §7.0). Local try still
does not need this.

```sh
# Creds (never commit): ~/.config/cloudflare/r2.env (chmod 600) or env
#   R2_ACCOUNT_ID
#   R2_ACCESS_KEY_ID
#   R2_SECRET_ACCESS_KEY
# Optional: R2_ENDPOINT, R2_BUCKET (default k2-web-bundles)

set -a; source ~/.config/cloudflare/r2.env; set +a   # load without printing
bun run vite:build:web
bun run web:publish                  # or: bash scripts/publish-web-bundles.sh
# bash scripts/publish-web-bundles.sh --dry-run
# bash scripts/publish-web-bundles.sh 0.40.53 --prune
```

| Piece | Value |
|---|---|
| Script | `scripts/publish-web-bundles.sh` (`bun run web:publish`) |
| Layout | `app/<ver>/index.html` + content-hashed assets under `app/<ver>/` |
| Cache-Control | `public, max-age=31536000, immutable` (set at upload) |
| Bucket | `k2-web-bundles` |
| Prune floor | `K2_WEB_BUNDLE_MIN_VERSION` (default `0.40.0` = loader `MIN_SUPPORT_VERSION`) |

**GitHub Actions secrets** (same names as env — do not put secrets in the repo):
`R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, optional
`R2_ENDPOINT` / `R2_BUCKET`.

`scripts/release.sh` calls the publish script after the desktop/daemon artifacts
are ready (Step 8.6). Without creds it **warns and continues** so a laptop
release is not blocked by R2; set `K2_REQUIRE_WEB_PUBLISH=1` to make upload
failure a hard gate. Optional prune on release: `K2_WEB_BUNDLE_PRUNE=1`.

If the first upload hits a TLS handshake error against
`*.r2.cloudflarestorage.com`, retry a few times (brand-new R2 endpoints can
warm slowly). That is not a credential problem.

---

## Quick checklist

```sh
# 1. Daemon up; confirm port file
cat ~/.k2/heartbeat.port

# 2. Owner login exists
k2 users add you --role owner

# 3. Build + serve
bun run vite:build:web
bun run web:serve

# 4. Browser
open http://127.0.0.1:8080/
```

Optional env:

| Variable | Role |
|---|---|
| `K2_DAEMON_PORT` | Daemon listen port (overrides heartbeat file) |
| `K2_WEB_PORT` | Caddy listen port (default `8080`) |
| `K2_WEB_VERSION` | SPA version dir under `out/web/app/` (default: `package.json` version) |
| `K2_HEARTBEAT_PORT_FILE` | Alternate path to the port file |

---

## Related

- PRD: [`prd/prd-hosted-web-client-and-edge-delivery-v1.md`](../prd/prd-hosted-web-client-and-edge-delivery-v1.md)
- Scripts: `scripts/web-serve.sh`, `web/Caddyfile`, `web/loader/`
- Vite web config: `vite.config.web.ts`
- Boot host force: `src/renderer/web/boot-host.ts`

## Phase 2 notes (daemon + SPA)

- **Cookie auth:** login with `web: true` / `X-K2-Client: web` sets `k2_session`
  (HttpOnly). SPA HTTP uses `credentials: include` and omits `?token=` on
  `/cli/*`. WebSockets still pass an in-memory token query pragmatically.
- **Owner wall:** `webClientEnabled` defaults **ON**. Disable with settings
  update `{"webClientEnabled":false}` (owner/admin). Web requests then get
  `WEB_CLIENT_DISABLED`; `/boot-status` still returns `webClient.enabled`.

