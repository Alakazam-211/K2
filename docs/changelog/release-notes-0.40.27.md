# K2 0.40.27 — See who's here

Presence & multiplayer V1 (PRD: `.k2/prds/prd-presence-multiplayer-v1.md`).

## Presence

- **Daemon presence registry.** The daemon now knows who is connected:
  every `/cli/sessions/events` WebSocket registers its resolved identity
  (owner token → Owner; connect-session token → username + role) and the
  workspace paths it subscribes to. Roster is broadcast whole-set as a new
  app-level `presence_changed` event and snapshottable via
  `GET /cli/presence/roster`. Liveness: server-originated WS ping (10s,
  2 misses = gone); entries deregister via drop-guards on every exit path.
- **Top-bar roster + presence modal.** ≤10 role-color-bordered initial
  avatars + `+N` (hidden when alone or the daemon predates presence);
  the modal lists every user with role chip, window count, workspaces,
  connected-since — and holds the moderation controls. Graceful 404
  degradation against older daemons, self-healing on upgrade.
- **Workspace-nav presence.** Sidebar rows (project, worktree, section)
  show ≤3 mini avatars + `+N` of who's in that workspace, replacing the
  git ± counters (the AI-Commit hover entry point is retired with them;
  the git data pipeline stays for the Changes panel, dirty dots, and
  branch labels). Path matching mirrors the daemon's
  `event_matches_workspace` rule exactly, nested worktrees included.

## Moderation & roles

- **Kick.** `POST /cli/presence/kick` (owner/admin, `can_act_on` matrix;
  admins cannot kick admins): revokes the target's persisted sessions AND
  fires their live connection close handles — immediate, no re-auth wait.
  Kick button (with confirm) in the presence modal, self-gated by role.
- **Viewer role.** New lowest tier (`viewer < member < admin < owner`),
  selectable in Settings → K2 Connect (capability-gated for old daemons).
  Viewers watch; they cannot type, resize, or claim terminals.
- **Edit grants.** `POST /cli/presence/grant` toggles a viewer's temporary
  edit capability; shown as a live "Edit" toggle on viewer rows in the
  modal. Grants attach to the live connection and auto-revoke when the
  user's last connection drops — re-grantable any time. Session records
  now carry ids (groundwork for per-window revocation later).

## Terminal multiplayer

- **Viewer/claimer enforcement (daemon-authoritative).** Grid connections
  resolve identity at accept and re-check claimer capability at every
  gate (mid-session role changes and grant revokes bite live sockets):
  viewer-mode connections get input dropped (one `input_denied` hint),
  resizes ignored (viewport still recorded for letterboxing), and claims
  refused — including the typing claim-steal. New `set_mode` message +
  `mode` ACK; non-owner connections default to viewer.
  NOTE: stream-token (per-session) grid connections are claimer-capable
  but also default to viewer mode — external clients driving a PTY over
  the grid WS must send `set_mode` (the sandboxes ops API is unaffected).
- **Per-window mode toggle.** Top-bar eye/edit toggle (main + focus
  windows) flips this window between viewer and claimer; disabled with an
  explanation when the daemon reports the connection can't claim.
  A "view-only" pill appears on panes when input is being refused.
- **Pin-to-size.** `POST /cli/terminal/pin-size` pins a session to fixed
  cols×rows, clamped at the daemon's single resize chokepoint (covers
  window resizes, typing claim-snaps, SetActive, detach-promotion).
  Persisted in `workspace_tab_sessions` (migration 0065) and restored on
  spawn; broadcast to subscribers (`pin_initial`/`pin_changed`). Tab menus
  (split panes AND single-pane/workspace tabs, incl. the pinned chat)
  offer 80×24 / 100×30 / 120×36 / 160×48 / match-my-window / unpin.
  Pinned panes letterbox for everyone — the active viewer too — with the
  scale floor relaxed to 0.25 while pinned; a "⌀ C×R" badge shows the pin.

## Also

- **Timer → stopwatch.** Countdown presets, extend-timer dialog, 3-2-1
  overlay and its themes are removed; a single play button starts a
  count-up timer. Pause/resume/stop, the session-memo prompt, cross-window
  sync, and the time-entries history/export are unchanged.
- Upgrade note: **older-version clients connecting to a 0.40.27 host
  appear in presence but default to viewer mode and cannot flip to
  claimer** (no `set_mode` support) — members on old clients are
  effectively view-only until they update their app.
- Test surface: +64 renderer tests (1040 total), 4 new daemon integration
  suites (presence, kick, viewer/claimer, pin-size), migration 0065.
