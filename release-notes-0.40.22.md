# K2 0.40.22 — Kessel: a first-class terminal

The terminal stack has a name now: **Kessel** — K2's daemon-authoritative,
multi-viewer terminal engine, rebuilt this release to feel first-class while
keeping the property that makes it unusual: one live session, watched from any
number of screens.

## Kessel — rendering & interaction

- **Pixel-smooth scrolling** via a transform-based row strip with overscan and
  an overlay scrollbar. Wheel scrolling paces to the display instead of a 20 Hz
  timer — in shells *and* in fullscreen TUIs (SGR wheel forwarding is now
  frame-paced behind a token bucket instead of a 50 ms flush).
- **Frame-paced emitter, serialized off the Term lock.** JSON is serialized
  outside the Term critical section so a full snapshot no longer stalls the PTY
  reader; emit cadence keeps an immediate-when-idle / 16 ms-coalesce-when-bursty
  floor. Cadence, frame-shape (Full/Delta), and DEC-2026 atomicity are now
  regression-pinned.
- **Resize no longer flashes black.** Daemon-side resize-settle suppresses the
  cleared-grid intermediate; client-side hold-and-scale keeps the last good
  frame stretched to the new box until real content arrives (content-gated, not
  geometry-gated, so it holds even against pre-settle daemons).
- **k1 binary wire (opt-in).** A compact binary frame format (`&proto=k1`)
  alongside JSON — ~3.4× smaller deltas, ~7.8× smaller full snapshots — with
  per-connection ack-gated pacing and snapshot resync for slow links. Old
  clients keep JSON unchanged.
- **Correct copy.** Wrapped rows carry a `wrapped` flag and rejoin without hard
  newlines; wide (CJK/emoji) and zero-width runs carry a column span so grid
  alignment and column math are correct; trailing padding is trimmed except on
  wrapped rows (fixes the "has a" → "hasa" boundary-space bug). Copy is rebuilt
  from the grid model, not the DOM.
- **TUI mouse interaction.** In a mouse-reporting fullscreen app, press/drag/
  release are forwarded as SGR so click-to-position, drag-select, and
  highlight-delete work; Shift/Option-drag overrides to K2-native selection;
  Cmd stays for links; the pointer flips I-beam/arrow to signal who owns the
  click.
- **OSC 52 clipboard.** A hosted app's clipboard writes are surfaced as a
  daemon event and applied at the active viewer only (deduped, size-capped,
  copy-direction only — read-back is never implemented).
- **Answer hosted TUIs' terminal queries.** Cursor-position (`?6n`), device
  attributes, kitty-keyboard negotiation and OSC color queries are now routed
  back to the app (`PtyWrite` → PTY) instead of dropped — hosted agents were
  talking to a terminal that never answered.
- **Experimental WebGL2 painter.** Settings → Terminal → Terminal Painter →
  WebGL: an instanced GPU painter (white-glyph atlas, per-row damage cache,
  custom selection) behind a flag, default DOM, with automatic DOM fallback on
  context loss.

## Kessel — session lifecycle & multi-viewer

- **Instant workspace switching.** Pinned chats of active workspaces stay warm
  in the background (MRU-bounded, cap grows with pinned-to-top count), so
  switching — including on remote servers — is a re-parent, not a reload.
- **Resize arbitration + restore-on-detach.** When the active viewer leaves,
  the PTY restores to the most-recent surviving viewer's size; resizes coalesce;
  same-size resizes are skipped (they blank TUIs). Sessions keep their
  last-visited size while parked — nothing is relinquished on leave.
- **No workspace-switch zoom.** Background/retained panes never emit resize; the
  hidden host mirrors the visible slot's dimensions; portal moves don't animate.

## Files

- **Uploads over 100 MB.** Streaming chunked uploads (shared with the existing
  clone-to transfer path) lift the cap to a 10 GiB running ceiling with a
  disk-space pre-check and a progress overlay.
- **Server-side compress + download.** Right-click a remote folder to zip it
  (streaming, Finder-style collision names, cancellable job); right-click a file
  to stream it to `~/Downloads` at any size.
- **Clone a workspace to your computer.** When viewing a remote server, "Clone
  to This Computer" pulls a packed workspace bundle down with your viewer
  credentials and imports it locally (files + chats + slug-rebased history) —
  no reverse connection required.

## Connect & federation

- **Reconnect after server update, finished.** Stale-credential recovery: the
  daemon 403s a dead connect-session, so the client now classifies that as an
  auth failure (not unreachability) and re-authenticates in-flight
  (single-flight), with a three-state surface — *restarting* (boot-status
  aware) → *re-authenticating* → *sign-in required* only if stored credentials
  are genuinely rejected.
- **Host-switch dashboard desync fixed.** Switching onto a remote whose session
  died no longer strands the dashboard on the old host; in-flight old-host
  responses can't clobber the new host.
- **Federation settings consolidated + gated.** "Enable federation" and "Let
  remote users message agents" now sit together under Remote access; both are
  restricted to Owner/Admin server-side (a Member could previously flip
  `federationEnabled` via the settings route).
- **Outbox actually delivers.** Messages queued for an offline peer now drain on
  the peer's next reachability, on a backoff sweep, and at boot — in-order,
  single-flight, with dead-lettering for declines and a queue cap. Added durable
  receiver-side dedupe (by signed message id) after discovering the prior
  nonce-based replay guard could drop a redelivered message.

## Stability

- **Terminal session leak fixed (three layers).** (1) Layout restore no longer
  re-mints terminal ids, and self-echoed `TabOrderChanged` no longer triggers a
  rebuild — two clients on one server could otherwise ping-pong fresh bare-shell
  spawns. (2) The PTY master is now released when a child exits (v2 never sent
  alacritty's shutdown message, so IO threads parked forever holding the fd).
  (3) Closing a tab reliably ends its daemon session (A6 contract, with a drift
  guard for unknown renderer values); `subscriber_count` counts real viewers,
  not internal observers. Daemon-side defense: a per-workspace cap on
  never-attached bare-shell sessions.
- The forever busy-spinner is fixed (pane unmount writes idle + a stale-working
  sweep, until detection moves daemon-side in a later release).

## Activity spinner

- **Busy spinner clears when the agent is done.** Fixed a case where a
  workspace's activity spinner could spin forever after switching away from it
  mid-task.

## Known / deferred

- Occasional screen tearing during fast scrolling is a compositor artifact under
  investigation; the WebGL painter avoids it.
- Daemon-side activity detection, per-peer federation trust/revoke UI, and TUI
  focus/hover modes are tracked for a later release.
