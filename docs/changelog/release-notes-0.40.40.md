# K2 0.40.40

Hotfix on `hotfix/chat-styles-shortcuts` (from main). Five focused fixes.

## Fixes

### Project chat permissions + attribution
- UI: composer gated on Connect **role** (Owner / Admin / Member can post;
  Viewer read-only) — no longer blocked by presence window-mode.
- Daemon: `POST /cli/project-group/msg` requires ≥ Member; Viewers get 403.
- Human posts store the real session author (username / owner), not always
  `"owner"`.

### Styles are per-client
- Style selection SSOT is localStorage; daemon is no longer canonical.
- Host switch does not restyle the client.
- One-shot migrate from daemon style when local mirror is empty.
- Multi-window sync via `storage` events.

### Cmd+N single-fire
- File menu accelerators that duplicated `useTerminalShortcuts` chords
  (N / T / Shift+T / D / O / W) removed; webview is the keyboard owner.

### Projects wake → Active
- Dashboard (and Feedback) wake / live attach call `activateProject` for
  the member workspace before `ensure-pinned-chat`, so Active reaper
  spares the session (~15s grace no longer fires while watching).

## Commits
- fix(projects): gate project chat post on Connect role (not window-mode)
- fix(daemon): project chat msg requires ≥ Member + real author attribution
- fix(styles): per-client Style selection — stop daemon SSOT + migrate local
- fix(shortcuts): Cmd+N creates one note — drop menu accelerators that double-fire
- fix(projects): activate member workspace on dashboard wake so Active reaper spares it
