# The K2 Agent Contract

How to make **any** coding agent — a commercial CLI, an open-source harness, or
your own program driving a local LLM — a first-class citizen of a K2 server.

K2 does not embed agents. Every agent is a **foreground CLI program running in a
PTY** inside a workspace, described by an **agent preset** (a row in the
`agent_presets` table: a command string plus declared metadata). If your program
honors the small contract below, it gets the full K2 experience: daemon-managed
spawn, prompt delivery, `k2` CLI participation (`respond`, `feedback`, `msg`,
`project`), API host sessions, and sane safety behavior.

Everything in this document describes what the daemon does **today** (0.40.30).
The final section lists what intentionally degrades for custom agents so you're
never surprised.

---

## 0. Where agents come from: presets and resolution

A workspace launches whatever its **default agent** resolves to, with 4-level
precedence (`crates/k2-core/src/workspace/agent_resolve.rs`):

1. An `AGENT.md` `launch:` block in the workspace (full manual override).
2. The workspace's own default (`projects.default_agent` — a preset id).
3. The global default agent (Settings).
4. The literal fallback: `claude --dangerously-skip-permissions`.

Levels 2–3 resolve against **enabled** presets. This is the classic footgun:
"hire a codex agent" without setting the workspace default silently launches
Claude at level 4. Fix it at hire time:

```bash
# Register/describe your agent program once…
k2 preset add --id my-agent --command 'my-agent --serve-loop' \
    --danger-flag --auto-yes --readiness settle:2000

# …then point workspaces at it (validates the preset exists + is enabled):
k2 agent hire ~/agents/scout --agent my-agent --template worker
```

`k2 preset list | show | add | set | remove` is the whole management surface
(also editable in Settings → Editors & Agents; mutations require the owner
token or an Owner/Admin session).

---

## 1. Spawn: a foreground CLI in a PTY

- K2 parses the preset's `command` string quote-aware into `argv`
  (`parse_command_string`) and spawns it in a PTY with
  **cwd = the workspace root**.
- Your program must run in the **foreground** and own the terminal until it
  exits. It must **not daemonize** — a program that forks and exits reads as a
  dead session.
- Interactive TUI or plain readline loop are both fine; see §3 for how prompt
  delivery differs.

## 2. Environment: what K2 stages for you

Every K2-spawned PTY gets, for free:

| Variable | Meaning |
|---|---|
| `K2_PORT` | Loopback port of the daemon's CLI API |
| `K2_HOOK_TOKEN` | Auth token for the `k2` CLI. By default (`K2_HOOK_SCOPED` on, opt out with `0`/`false`/`off`) every agent session gets a **per-session scoped token** here — it identifies *your session*, and is what `k2 respond` authenticates with. Never the daemon owner token. |
| `K2_PROJECT_PATH` | The workspace root |

So any process inside the PTY can already run `k2 respond`, `k2 tickets ask`,
`k2 msg`, `k2 project msg`, … — the `k2` CLI resolves its connection from these.

On top of that, the preset's **`env` metadata** (a JSON object) is merged into
the child environment at every daemon-initiated spawn. Precedence, highest
first:

1. `AGENT.md` `launch:` block env (a launch block replaces the profile
   wholesale — preset env never leaks under it),
2. K2-internal env (`K2_HOOK_TOKEN` and friends),
3. **preset `env`**,
4. inherited shell env.

Preset env values may hold secrets (API keys, base URLs); the daemon never logs
them. For API host sessions the caller's env is **dropped entirely** — preset
env is the base layer and K2-curated entries (e.g. the principal's
`ANTHROPIC_API_KEY`) override same-named preset entries.

```bash
k2 preset set my-agent --env OPENAI_API_BASE=http://localhost:8000/v1 \
                       --env OPENAI_API_KEY=not-needed
```

## 3. Prompt intake: text into the PTY, post-readiness

K2 delivers tasks (wake messages, `k2 msg`, API `prompt`s, feedback-thread
comments) by **writing text into your stdin/PTY and submitting with Enter**
(the injector, `workspace_msg::inject_and_submit`). Your program must:

- accept a task as a line (or bracketed-paste block) of text on stdin,
- tolerate **bracketed-paste framing** (`ESC[200~ … ESC[201~`), and
- tolerate a trailing **insurance Enter** (the injector may send a second
  Enter; line-oriented REPLs should treat an empty follow-up line as a no-op).

## 4. Readiness: declare it, don't make K2 guess

The injector needs to know when your input loop is live. Presets declare one of
two classes (migration-0070 `readiness` column):

- `bracketed-paste` — your TUI enables bracketed paste (`?2004h`) exactly when
  it is ready for input, and the signal is trustworthy (claude, grok,
  cursor-agent behave this way).
- `settle:<ms>` — your `?2004h` timing lies (or you never enable it); K2 should
  wait a settle floor after spawn/wake instead (codex and gemini are
  `settle:2000`, pi `settle:1500`, hermes `settle:7000`).

```bash
k2 preset set my-agent --readiness settle:2000
```

Every injection site resolves timing through one precedence chain
(`provider_resume::resolve_injection_profile`): **your declared `readiness`
wins** → the static table of audited providers → the safe default
(bracketed-paste poll + settle fallback) for unknown commands. Declared
settles are capped at 60s at consume time, so keep `settle:<ms>` under
60000 (the write-side accepts more, but it will be clamped).

## 5. Responding to API callers: `k2 respond`

When a session is launched through the K2 API (`POST /v1/w/<ws>/host-sessions`
or a sandbox cell), the caller is a **program** — it cannot see your terminal.
The response channel is:

```bash
k2 respond "progress: found the root cause"    # emit as often as you like
k2 respond --final "done: fix landed on main"  # mark the turn's last line
```

- Auth is the env-provided **scoped** `K2_HOOK_TOKEN` (shape `<sid>.<secret>`,
  minted per session) — over the per-cell UDS in sandboxes, or loopback
  `POST /cli/respond` for host sessions. Outside an API-launched session
  `k2 respond` fails loudly.
- The caller drains lines via `GET …/host-sessions/<id>/messages` (or the
  sandbox equivalent).
- K2 briefs your agent in-band: API-delivered prompts are prefixed with the
  `[K2 API]` preamble telling the agent to use `k2 respond` / `--final`. An
  agent that can read its prompt and run shell commands needs no prior
  knowledge of K2 to comply — but a bare model REPL (no tool use) cannot, see
  §8.

## 6. Permissions: declare your danger flags

Many agent CLIs have an "auto-approve everything" flag
(`--dangerously-skip-permissions`, `--yolo`, `--yes-always`, …). On the host —
where there is no microVM jail — your agent's own permission prompts **are** a
safety layer, so:

- Declare your agent's auto-approve flags in the preset:
  `k2 preset add --id my-agent --command 'my-agent --auto-yes' --danger-flag --auto-yes`.
- API host-session spawns keep your declared auto-approve flags by default
  (`api_skip_permissions` per workspace, **default ON** — `/v1` is headless and
  cannot answer a HITL gate). Owners who want fail-closed stripping can opt out:
  `k2 workspace api-skip-permissions set <ws> off`.
- When skip-permissions is OFF, host-session spawns **strip the union** of your
  declared flags and K2's audited floor from the argv. A preset with **no
  declared flags is treated as unknown, never as safe**: only the floor is
  stripped and the daemon logs the honest residual. Declaring is what closes
  that gap.

Owner-initiated spawns (tabs, wake, heartbeats) run the preset command as
configured — stripping applies to the API door.

## 7. Docs: read `AGENTS.md` in your cwd

K2 maintains a canonical `.k2/AGENTS.md` in every managed workspace, mirrored
to the harness entrypoints (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`,
`.cursor/rules`, `.goosehints`, aider conf). It contains the workspace context
**and** the K2 CLI teachings (the loadable `k2-cli` skill:
`.k2/skills/k2-cli/SKILL.md` — `msg`, `inbox`, `tickets`, `respond`,
`project`, heartbeats).

Contract item: **your agent should read `AGENTS.md` at cwd on startup** (the
emerging cross-tool convention). If it does, it self-discovers everything in
§5 without any prompt engineering from the caller.

## 8. Local LLMs: run a harness, not a bare REPL

A raw model REPL (`ollama run qwen3`) is **not** an agent: no tool use, no file
edits, it cannot run `k2 respond`. What works — today, with presets alone — is
any of the mature harnesses driving a local **OpenAI-compatible server**
(Ollama's `/v1`, LM Studio, vLLM, llama.cpp `--api`). Aider, Goose, OpenCode
and Open Interpreter are all seeded presets already; the preset `env` column is
where their base-URL configuration lives.

### Worked example: aider backed by Ollama

```bash
# 1. Serve a model locally.
ollama pull qwen3 && ollama serve      # OpenAI-compatible at :11434

# 2. Describe the agent to K2 — command + env + truthfully declared flags.
k2 preset add --id aider-ollama \
    --command 'aider --model ollama_chat/qwen3 --no-auto-commits' \
    --env OLLAMA_API_BASE=http://localhost:11434 \
    --danger-flag --yes-always \
    --readiness settle:2000

# 3. Hire a workspace onto it.
k2 agent hire ~/agents/local-coder --agent aider-ollama --template worker --launch
```

Notes:

- Aider reads `OLLAMA_API_BASE` for `ollama_chat/…` models; for any other
  OpenAI-compatible server use `--env OPENAI_API_BASE=http://host:port/v1`
  (plus `OPENAI_API_KEY=anything` if the server doesn't check keys). Goose and
  OpenCode have equivalent env-based provider config — put it in the preset the
  same way.
- Declare the harness's auto-approve flag (`--yes-always` for aider) even if
  you don't put it in the command — that's what keeps the API door fail-closed
  (§6).
- Aider consumes K2's docs fan-out via its conf `read:` entry (§7), so it
  learns `k2 respond` / `k2 tickets` without extra setup.

## 9. Conformance tiers (quick self-check)

| Tier | You have | You get |
|---|---|---|
| **Functional** | §1 spawn + §2 env + §3 prompt intake + §5 respond | Spawnable, promptable, answers API callers |
| **Safe** | + §6 declared danger flags | Correct fail-closed API + headless behavior |
| **First-class** | + §4 readiness + §7 AGENTS.md consumption | Snappy injection, self-documenting sessions |

## 10. Honest degrades (custom agents today)

- **No resume/continuity**: session-id premint/resume grammar is a static
  table for the known providers; a custom agent is fresh-per-spawn (pinned-chat
  continuity and `{"session": id}` API resume degrade gracefully).
- **Undeclared readiness** on a custom command falls to the default injection
  profile (§4) — safe but slower than a truthful declaration.
- **Sandbox cells** are host-fixed to headless claude; custom agents run as
  host sessions, not in cells.
- **HITL auto-answer** fast-paths cover the known dialogs; custom agents fall
  back to the local classifier or manual handling.

Design source of truth: `.k2/notes/custom-agents-local-llm-design.md` (gap
analysis + the build plan behind this contract).
