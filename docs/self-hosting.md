# Self-Hosting K2 on Linux

Run your own K2 server on any Linux box — a VPS, a home server, or a Raspberry Pi.
One command installs the daemon; then you pair the subdomain you bought on
[k2.dev](https://k2.dev) and you're live at `https://<you>.k2.dev`.

Requires K2 daemon **0.40.52 or newer**.

---

## What you need

- **A Linux box** — Ubuntu 22.04/24.04, Debian 12, or Raspberry Pi OS 64-bit — with
  root (or `sudo`).
- **A subdomain** purchased on k2.dev (get one at <https://k2.dev/dashboard>).
- **Outbound internet.** The daemon binds `127.0.0.1` only — all remote access runs
  through the encrypted K2 Connect tunnel, so you never open a port or expose the box.

---

## Quick start

Three steps: install, create your login, pair your subdomain.

### 1. Install the daemon + CLI

```sh
curl -fsSL https://raw.githubusercontent.com/Alakazam-211/K2/main/scripts/provision-k2-server.sh -o provision-k2-server.sh
chmod +x provision-k2-server.sh
sudo ./provision-k2-server.sh
```

This installs and starts the K2 daemon (with a signature-verified download) and the
`k2` CLI. It doesn't pair anything yet — that's the next step. Re-running is safe; the
script is idempotent.

### 2. Create your owner login

```sh
k2 users add <you> --role owner
```

Prompts for a password (hidden input). This is the account you'll sign in with.

### 3. Pair the subdomain you purchased

```sh
k2 connect login
```

- Sign in with your **k2.dev email + password** (or `k2 connect login --token <access-jwt>`).
- Pick your subdomain from the list.
- K2 writes the tunnel config, starts the tunnel, and prints your live URL.

Done — your server is live at **`https://<your-subdomain>.k2.dev`**.

---

## The tools

| Command | What it does |
|---|---|
| `provision-k2-server.sh` | Installs + starts the daemon and the `k2` CLI. Idempotent. |
| `k2 users add <name> --role owner\|admin\|member\|viewer` | Create a login on the server. `--password-stdin` for scripts. |
| `k2 connect login` | Pair a purchased subdomain: account sign-in → pick → live. |
| `k2 connect status` | Show the paired subdomain and plan. |
| `k2 connect logout` | Sign the box out of your k2.dev account. |

Your account session is stored locally at `~/.k2/connect-account.json` (mode `0600`);
**your password is never written to disk** — it's exchanged for a scoped access token.

---

## Automated / scripted setup

Deploying with cloud-init or a script, and already have your subdomain's tunnel token
from the dashboard? Skip the interactive step and pass everything via environment:

```sh
K2_TUNNEL_TOKEN=k2c_...          # your subdomain's tunnel token
K2_SUBDOMAIN=you \
K2_OWNER_USER=you \
sudo ./provision-k2-server.sh
```

---

## Verify

- `curl https://<your-subdomain>.k2.dev/boot-status` returns `{"phase":"ready", …}`.
- `k2 connect status` shows your subdomain.
- Sign in at your subdomain — or via the K2 desktop app's Remote Sign-In — with your
  owner login.

---

## Updating

The daemon self-updates in place, and the `k2` CLI updates along with it — nothing to
do by hand. Check the running version any time with `k2 --version`.

---

## Good to know

- The daemon runs as a **non-root** service user and binds `127.0.0.1` only; remote
  access is exclusively the tunnel. Don't "fix" this by exposing the port.
- This is a **Standard** setup — sandboxing is off.
- **Raspberry Pi** (aarch64) works with the exact same steps.
- If your account owns no subdomain yet, `k2 connect login` links you to the dashboard
  to buy one.
