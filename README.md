<p align="center">
  <img src="docs/hero.png" width="100%" alt="K2 by Alakazam Labs — Multiplayer agent orchestration">
</p>

<p align="center">
  <a href="https://github.com/Alakazam-211/K2/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/license-FSL--1.1--Apache--2.0-blue.svg" alt="FSL-1.1-Apache-2.0"></a>
  <a href="https://k2.dev"><img src="https://img.shields.io/badge/k2.dev-8B5CF6.svg" alt="k2.dev"></a>
  <a href="https://discord.gg/73b3sg6pSQ"><img src="https://img.shields.io/badge/Discord-K2%20Community-5865F2?logo=discord&logoColor=white" alt="K2 Discord"></a>
  <a href="https://github.com/Alakazam-211/K2/releases"><img src="https://img.shields.io/github/v/release/Alakazam-211/K2?display_name=tag&sort=semver" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg" alt="macOS | Linux">
  <img src="https://img.shields.io/badge/built_with-Tauri_v2-24C8D8.svg" alt="Tauri v2">
</p>

<p align="center">
  <strong>Multiplayer Agent Orchestration Platform</strong><br/>
  <em>Agents as infrastructure.</em> Host teams of agents that work together and run your business.
</p>

<p align="center">
  <a href="https://github.com/Alakazam-211/K2/releases/latest"><strong>Download</strong></a> ·
  <a href="https://k2.dev">k2.dev</a> ·
  <a href="https://discord.gg/73b3sg6pSQ">Discord</a> ·
  <a href="WHATS_NEW.md">What's New</a> ·
  <a href="https://k2.dev/docs/api">API</a>
</p>

---

# K2

**Host an agent team, not a single agent.**

K2 is **agent server software** — not a traditional IDE and not a model provider. Your agents run on **your machine** through the CLI tools you already use (Claude Code, Codex, Gemini, Pi, Hermes, and more), keep working when you close the window, and stay reachable from any device.

- **Your keys, your hardware** — bring your own model accounts
- **One daemon, many viewers** — desktop, remote web, mobile companion; the server is the source of truth
- **Multiplayer by design** — teams of people *and* teams of agents on the same server

Built with Tauri + Rust. Fair Source — free to use and self-host.

---

## What an agent has

On K2, an **agent** is a durable unit of work you host — not a chat tab that vanishes when you leave.

| Capability | What it means |
|------------|----------------|
| **Hands** | A real terminal (PTY) + filesystem on a project directory |
| **Identity** | Persona (`AGENT.md`) and standing orders you control |
| **Project knowledge** | Shared project truth (`PROJECT.md`) and optional layers |
| **Context management stack** | Lean always-on `AGENTS.md` composed from system + catalog layers |
| **Skills** | Loadable playbooks for depth (not dumped into always-on context) |
| **Heartbeats** | Scheduled wakes so work continues without you babysitting the window |
| **Connections** | Other agents it can discover, message, and hand work to |
| **API surface** | The same agent can be messaged or spawned via HTTP when you expose the server |

Hire one with `k2 agent hire`, shape its always-on context with the **context management stack**, launch any CLI harness with **agent presets**, and operate day-to-day with `k2 agent …`.

---

## Pillars

### 1. Agents as infrastructure

Stand up agents the way you’d stand up services: name them, give them a home directory, wire connections, schedule heartbeats, and leave them running under the **daemon** — not under a single app window.

### 2. Multiplayer agent management

One K2 server is a place **your team logs into**. Presence, roles (Owner / Admin / Member), shared live terminals, grant-the-keyboard, and audit trails. People and agents collaborate in the open.

### 3. Teams of agents that run the business

Agents **message each other** (`k2 msg`, inbox), stay **connected** across projects, and coordinate without a human relay. Cross-repo work becomes “message the other agent,” not copy-paste between chats.

### 4. Context management stack

Always-on context is a **stack of markdown layers** composed into `.k2/AGENTS.md`:

- **System:** persona, project knowledge, tooling pointer  
- **Optional:** wiki packs, role packs, live rosters (connections / heartbeats / skills)  
- **Day-2:** Settings + `k2 agent context …` + local **catalog** (`k2 agent context catalog`)

Keep the stack lean; load **skills** for depth. Harnesses symlink to the same generated entrypoint.

### 5. Any CLI agent (your subscription)

If it runs in a terminal, it runs on K2 — no wrapper SDK required.

Claude · Codex · Gemini · Copilot · Grok · Cursor Agent · OpenCode · Goose · Pi · Hermes · **+ custom presets**

Manage the launch roster with `k2 preset`; point a hire at one with `--agent`.

### 6. Daemon-first, reachable from anywhere

- **Headless daemon** on macOS or Linux — sessions keep running when the UI is closed  
- **[K2 Connect](https://k2.dev/k2-connect)** — secure tunnel, `your-name.k2.dev`, remote files, clone-to, multi-user  
- **Companion apps** — monitor and chat from your phone  

### 7. Agents as API endpoints

K2 can **serve** the agents you host:

- Message a real workspace agent from CI, cron, or GitHub Actions  
- Spawn sandboxed agent sessions over HTTPS  
- Watch API-started work as live tabs in the app  

Same agents you operate in the UI — addressable with a key and `curl`. See [API docs](https://k2.dev/docs/api).

```bash
# Message a workspace agent from your pipeline
curl -X POST "https://your-name.k2.dev/v1/w/api/message" \
  -H "Authorization: Bearer $K2_KEY" \
  -d '{"text":"deploy when tests pass","from":"CI bot"}'
```

---

## Quick start

### Download

- **[Latest release](https://github.com/Alakazam-211/K2/releases/latest)** (macOS; Linux daemon beta on [k2.dev/download](https://k2.dev/download))  
- Product site: **[k2.dev](https://k2.dev)**  
- Community: **[Discord](https://discord.gg/73b3sg6pSQ)**

### Build from source

**Prerequisites:** [Rust](https://rustup.rs/) (stable), [Bun](https://bun.sh/) (or Node 18+), cmake, Xcode CLT (macOS).

```bash
git clone https://github.com/Alakazam-211/K2.git
cd K2
bun install
cargo tauri dev
```

Release build: `cargo tauri build`.

### Day-2 CLI (taste)

```bash
k2 agent hire ~/agents/ops --name "Ops" --context wiki:hygiene --context connections:roster
k2 agent context catalog
k2 agent context add manager:pack
k2 connections list
k2 msg mobile-app "API cursors shipped — update the client"
```

---

## Under the hood (short)

| Layer | Role |
|-------|------|
| **Daemon** (`k2-daemon`) | Canonical state, heartbeats, terminals, HTTP/WS, headless server |
| **Desktop / web clients** | Thin viewers — connection + OS integration, not a second brain |
| **`k2` CLI** | What agents and humans use for msg, context, hire, presets, … |
| **Stack** | Tauri v2, React, Rust, SQLite, Alacritty-class terminals |

Full detail: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Community

- **[Discord](https://discord.gg/73b3sg6pSQ)** — K2 community  
- **[GitHub Issues](https://github.com/Alakazam-211/K2/issues)** — bugs & features  
- **[What's New](WHATS_NEW.md)** — product highlights by version  
- **[Contributing](CONTRIBUTING.md)** — dev setup, presets, styles  

---

## Fair Source

[FSL-1.1-Apache-2.0](LICENSE.md) — free for personal and internal/business use; each release converts to Apache 2.0 two years later. Source is fully visible.

Hosting K2 commercially for others is covered **via the official K2 Connect tunnel** — see [COMMERCIAL_HOSTING_GRANT.md](COMMERCIAL_HOSTING_GRANT.md).

---

## Developing & testing

See [CONTRIBUTING.md](CONTRIBUTING.md). Smoke tests live under `tests/` (CLI + behavior tiers); run against a local daemon / `cargo tauri dev` as documented there.

```bash
cargo tauri dev
./tests/cli-integration-test.sh
./tests/behavior-test-tier1.sh
./tests/behavior-test-tier3.sh
```
