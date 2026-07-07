# K2 0.40.30 — Talk to your server, not just at it

The API release: spawn real agent sessions over HTTP, get answers back, and
bring any agent — including custom/local-LLM agents — as a first-class
citizen. Full API reference: https://k2.dev/docs/api

## Host-sessions API (/v1)

- `POST /v1/w/<ws>/host-sessions` boots the workspace's configured agent in a
  real terminal with your prompt; the session answers back via `k2 respond`
  (`--final` marks the answer). `GET …/messages?since=<seq>` reads replies on
  a stable, non-destructive cursor.
- List, resume, and follow-up message endpoints. Resume of a live session
  coalesces into live delivery (never double-boots); resume of a dead one
  relaunches with the provider's own resume grammar.
- Every API-spawned session is briefed on the respond contract (the `[K2 API]`
  preamble), so agents know a caller is listening — no guesswork.
- Session identity is table-driven across provider grammars: premint
  (Claude/Grok), post-boot adoption (Codex/Gemini/Pi/Cursor/Hermes). Adapter-
  less presets spawn + message but are honestly absent from list/resume.
- Safe by default: agents' auto-approve flags are STRIPPED on API spawns
  unless the workspace opts in via `api_skip_permissions` (migration 0069).
  Stripping is data-driven and fail-closed from preset metadata.
- Readiness-aware prompt delivery: injection honors per-provider startup
  profiles (and preset-declared `readiness`), so slow-booting TUIs don't eat
  the first message.
- API-key principals stage provider-mapped credentials (OpenAI/Gemini/xAI +
  `OPENAI_BASE_URL`), not just Anthropic (migration 0071).
- Stream-token grid connections default to claimer mode.

## Custom agents / any-agent

- Agent presets carry metadata: `danger_flags`, `env`, `readiness`
  (migration 0070), with truthful seeds for all 13 built-ins.
- New `k2 preset` CLI: list/show/add/set/remove — manage custom agents
  headlessly. `k2 agent hire --agent <preset>` sets the workspace default
  (hiring a Codex agent now launches Codex, not the fallback).
- `docs/agent-contract.md`: what a custom or local-LLM agent must do to be
  first-class in K2.
- Sandbox cells launch the workspace's real preset (Claude argv byte-
  identical); guest-init honors host-resolved argv; guest image recipe adds
  the npm-installable agent CLIs (validated at next Linux bake).

## K2 Cloud / fleet

- `K2_API=1` + `K2_HOOK_SCOPED=1` in the Standard provision template — new
  servers get the message API and respond read-back out of the box.
- Owner-role gates for API-key management and tunnel-config writes; optional
  `K2_OPS_USER` website-management service user (credential via callback
  only); must-change-password sessions + consume-once seed-users.
- Tunnel lease blob fix; minisign verification accepts both signature forms.

## Fixes

- Dev builds tolerate daemon version skew instead of kickstart-looping the
  live daemon (killed sessions on every `tauri dev` launch).
- Renderer timer-test mock typing (typecheck:web unmasked).
