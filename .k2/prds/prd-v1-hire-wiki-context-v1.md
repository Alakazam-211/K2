# Mini-PRD — `/v1` hire + wiki notes + context layers

**Date:** 2026-08-24 · **Owner:** Rosson · **Status:** On main `5facba61` (unreleased)  
**Product:** K2 daemon `/v1` (API key / owner token)  
**Amends:** wiki `Research - Scout Hire APIs` items **2–4** (item **1** VM carve stays k2-dev-web)

Interview lock: N host-session cells for scripts; **per-interview workspace** when hire lands. No `instructions` field. A draft is not this slice.

---

## 1. Problem

Julie/Scout can spawn host-sessions with an API key. They cannot **create** a workspace, **write wiki notes**, or **stack context** without the daemon owner token (`k2 agent hire` / `/cli/context/*` / FS). Hire today is CLI orchestration over `/cli`.

## 2. Locked product

| # | Verb | Door |
|---|------|------|
| **2** | Create / converge a workspace (hire) | `POST /v1/w` |
| **3** | Write a wiki note under `.k2/wiki/` | hire body `wiki[]` **and** `POST /v1/w/<ws>/wiki/notes` |
| **4** | Stack catalog ids and/or inline layer markdown; regen | hire body `context`/`layers` **and** `POST /v1/w/<ws>/context` (+ remove/regen) |

Not v1: VM carve, Graph, `instructions`, auto-`--launch`/`--onboard` (interviews use host-sessions). Catalog **authoring** (`/cli/context/catalog/create`) stays owner/manage.

## 3. Decisions

| # | Decision |
|---|----------|
| **D1** | **`POST /v1/w`** (exact path, no slug) is hire. Body JSON camelCase. Required: `path` (abs or `~`). Optional: `name`, `preset` (`default_agent`), `template` (worker\|manager\|qa\|researcher) **XOR** `persona` (markdown string, not a host file path), `wiki` `[{id, body}]`, `context` (catalog id strings), `layers` `[{label, markdown}]`, `defaultModel`, `noWiki` (default **false** → seed like CLI). Idempotent: same `path` already registered → converge (no 409). Do **not** spawn a PTY. |
| **D2** | **Authz.** Same surface gate as host-sessions (`K2_API`). Capability: `host-sessions`. **New** path: owner **or** API key whose grant is `*` (finite slug list cannot mint unknown workspaces — 403 usage, not a 404 oracle). **Existing** path: `resolve_authorized_workspace` (uniform 404 if ungranted). Wiki/context mutations on `<ws>`: same grant + cap as host-sessions (uniform 404). |
| **D3** | **Wiki notes.** `id` is wiki-rel (`Interview.md`, `foo/bar.md`). Jail: no `..`, no abs, must be `.md`, under `.k2/wiki/` only. Max body `wiki::MAX_NOTE_BYTES` (1 MiB). Seed vault if missing. Overwrite OK. Response `{ok, id, path}`. |
| **D4** | **Context.** `POST /v1/w/<ws>/context` body: `catalog` **or** `{label, markdown}` (exactly one). Markdown writes `.k2/context/<slug>.md` then `add_layer(path=…)`. `POST …/context/remove` `{path\|catalog\|id}`. `POST …/context/regen`. `GET …/context` = layer list (parity with `/cli/context/layers`). After mutate: `charter_compose_watch::resync_watches`. Workspace-scoped only (no per-session layers). |
| **D5** | **Reuse core.** `lifecycle::create_workspace_ex` / open-if-registered, `workspace::agent::create`, display-name + `default_agent` + `default_model` via existing project update allowlist, `wiki::seed_wiki`, `context_layers::{add_layer,remove_layer,regen}`. Emit `ProjectsChanged` + `fs_live`/`charter` resync on hire. POST-only mutations (`if !is_post { 405 }`). Dispatcher: allow POST `/v1/w` (today only `/v1/w/…` prefix). |

**Out of v1:** `--launch` / `--onboard` / `--connect` / `--project` / `--fanout`. Finite-list keys auto-appending the new slug onto `allowed_workspaces` (follow-up if Julie’s keys are not `*`).

---

## 4. Tests (loud)

| Case | Expect |
|---|---|
| Owner `POST /v1/w` `{path}` | 200; project row; wiki Home seeded |
| API key grant `*` + cap host-sessions | 200 hire |
| API key finite list, new path | 403 usage (cannot create) |
| API key no host-sessions cap | uniform 404 |
| Re-hire same path | 200 `changed:false` or equivalent converge |
| `wiki: [{id:"X.md", body}]` on hire + later `POST …/wiki/notes` | file under `.k2/wiki/X.md` |
| `..` / abs wiki id | 400 usage |
| `context: ["wiki:hygiene"]` | layer stacked |
| inline `layers` | file + layer |
| GET `/v1/w` | 405 |
| K2_API off | 404 surface-absent |

No `unwrap_or` in assertions. No skip-if-missing. No live Scout in CI.

## 5. Success

Julie, with an API key (`*` grant, host-sessions cap) against an already-running Scout daemon:

```
POST /v1/w  {path, name, preset, wiki:[{id,body}], context:[…], defaultModel}
POST /v1/w/<handle>/host-sessions  {prompt, model?}
```

writes the workspace, wiki, and layers without the owner token. VM carve remains k2-dev-web.
