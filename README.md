<p align="center">
  <a href="https://k2.dev">
    <img src="docs/hero.png" width="100%" alt="K2 by Alakazam Labs — Multiplayer Agent Management Platform">
  </a>
</p>

<p align="center">
  <a href="https://github.com/Alakazam-211/K2/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/license-FSL--1.1--Apache--2.0-blue.svg" alt="FSL-1.1-Apache-2.0"></a>
  <a href="https://k2.dev"><img src="https://img.shields.io/badge/k2.dev-8B5CF6.svg" alt="k2.dev"></a>
  <a href="https://discord.gg/73b3sg6pSQ"><img src="https://img.shields.io/badge/Discord-K2%20Community-5865F2?logo=discord&logoColor=white" alt="K2 Discord"></a>
  <a href="https://github.com/Alakazam-211/K2/releases"><img src="https://img.shields.io/github/v/release/Alakazam-211/K2?display_name=tag&sort=semver" alt="Latest release"></a>
  <a href="https://github.com/Alakazam-211/K2/stargazers"><img src="https://img.shields.io/github/stars/Alakazam-211/K2?style=flat" alt="GitHub stars"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg" alt="macOS | Linux">
  <img src="https://img.shields.io/badge/built_with-Tauri_v2-24C8D8.svg" alt="Tauri v2">
</p>

<h1 align="center">K2</h1>

<p align="center">
  <strong>Multiplayer Agent Management Platform</strong><br/>
  <em>Agents as infrastructure.</em><br/>
  Host teams of agents that work together and run your business.
</p>

<p align="center">
  <a href="https://github.com/Alakazam-211/K2/releases/latest"><strong>⬇ Download</strong></a>
  &nbsp;·&nbsp;
  <a href="https://k2.dev">Website</a>
  &nbsp;·&nbsp;
  <a href="https://discord.gg/73b3sg6pSQ">Discord</a>
  &nbsp;·&nbsp;
  <a href="WHATS_NEW.md">What's New</a>
  &nbsp;·&nbsp;
  <a href="https://k2.dev/docs/api">API</a>
  &nbsp;·&nbsp;
  <a href="https://k2.dev/k2-connect">K2 Connect</a>
</p>

---

## Host an agent team, not a single agent

K2 is **agent server software** — not a traditional IDE, not a model provider.

Your agents run on **your machine** with the CLI tools you already use. They keep working when you close the window. Your team can log in, watch live, and manage the fleet from anywhere.

**Your keys. Your hardware. Full visibility.**

---

## What you get

### Agent teams on your server

Stand up a **team of agents**, not one disposable chat. Name them, house them on real projects, leave them running under a daemon that doesn’t sleep when the UI closes.

### Multiplayer — people and agents

One server is a place **your team logs into**. Presence, roles, shared live terminals, grant-the-keyboard, audit. Humans manage; agents work; everyone sees the same truth.

### Agents that work together

Agents discover each other, **message** across projects, and hand off work without you as the relay. Cross-repo collaboration becomes “tell the other agent,” not copy-paste between windows.

### Any coding agent you already pay for

Claude Code, Codex, Gemini, Copilot, Grok, Cursor, OpenCode, Goose, Pi, Hermes — **if it runs in a terminal, it runs on K2.** Bring your own subscriptions. No wrapper SDK required.

### Always-on context, one truth for every harness

Project knowledge and agent persona compile into a single always-on entrypoint every tool can read. Edit once — Claude, Codex, Cursor, and the rest stay in sync. Keep it lean; load skills when you need depth.

### Reach your machine from anywhere

**[K2 Connect](https://k2.dev/k2-connect)** tunnels your daemon to `your-name.k2.dev`. Remote workspaces, files, clone-to, multi-user access. Companion apps for phone. Same terminals, live — from the train.

### Agents as API endpoints

Build agents on K2, then **serve** them. Message a real agent from CI or cron. Spawn sandboxed sessions over HTTPS. Watch API-started work as live tabs in the app.

```bash
curl -X POST "https://your-name.k2.dev/v1/w/api/message" \
  -H "Authorization: Bearer $K2_KEY" \
  -d '{"text":"deploy when tests pass","from":"CI bot"}'
```

[API docs →](https://k2.dev/docs/api)

### Terminal-first workspace

Serious terminals, worktrees, files, review, and styles — built to manage agents, not to replace your editor of choice. Open anything in Cursor/VS Code when you want; K2 stays the control plane.

### Daemon-first (close the laptop)

Agents run against the **K2 daemon**, not the window. Shut the lid — sessions keep going. Every viewer reconnects to the same live state.

---

## Quick start

| | |
|---|---|
| **Download** | [Latest release](https://github.com/Alakazam-211/K2/releases/latest) · [All platforms](https://k2.dev/download) |
| **Product** | [k2.dev](https://k2.dev) |
| **Community** | [Discord](https://discord.gg/73b3sg6pSQ) |

### Build from source

**Prereqs:** [Rust](https://rustup.rs/) (stable), [Bun](https://bun.sh/) (or Node 18+), cmake, Xcode CLT (macOS).

```bash
git clone https://github.com/Alakazam-211/K2.git
cd K2
bun install
cargo tauri dev
```

Release: `cargo tauri build`.

---

## Under the hood

| Piece | Role |
|-------|------|
| **Daemon** | Source of truth — always on, headless-capable |
| **Desktop / web / mobile** | Thin viewers on the same server |
| **`k2` CLI** | Day-to-day surface for humans and agents |
| **Stack** | Tauri v2 · React · Rust · SQLite |

Deep dive: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## What K2 is not

- **Not an IDE replacement** — agent management infrastructure with a serious terminal UI  
- **Not a model company** — bring Claude / Codex / Gemini / … yourself  
- **Not “only while the window is open”** — the daemon owns the loops  

---

## Community

- **[Discord](https://discord.gg/73b3sg6pSQ)** — K2 community  
- **[Issues](https://github.com/Alakazam-211/K2/issues)** — bugs & features  
- **[What's New](WHATS_NEW.md)** — product highlights by version  
- **[Contributing](CONTRIBUTING.md)** — dev setup  

---

## Fair Source

[FSL-1.1-Apache-2.0](LICENSE.md) — free for personal and internal/business use; each release converts to Apache 2.0 two years later.

Commercial multi-tenant hosting via official **K2 Connect** — [COMMERCIAL_HOSTING_GRANT.md](COMMERCIAL_HOSTING_GRANT.md).

---

<p align="center">
  <sub>
    <strong>K2</strong> by Alakazam Labs ·
    <a href="https://k2.dev">k2.dev</a> ·
    <a href="https://discord.gg/73b3sg6pSQ">Discord</a> ·
    <a href="https://github.com/Alakazam-211/K2/releases/latest">Download</a>
  </sub>
</p>
