# K2 0.40.25 — Any agent, first-class

## Multi-agent K2 (agent de-generalization)

K2 is no longer a Claude app with other agents bolted on. Everything that
used to assume Claude now works for the Big 7 — Claude, Codex, Gemini,
Grok, Cursor Agent, Pi, Hermes — with safe degradation for anything else.

- **Default agent, global and per-workspace.** The Editors & Agents default
  actually works now (it silently fell back to Claude for ~10 consumers);
  each workspace gets its own "Default Agent" dropdown (inherit-global by
  default, stamped at creation, non-retroactive). Cmd+Shift+T, heartbeat
  launches, headless wakes, agent launches, and restart recovery all honor
  AGENT.md `launch:` → workspace default → global default → claude.
- **Multi-agent canonical sessions.** The pinned tab's session picker lists
  every provider's sessions with brand icons; pick any one and it becomes
  the workspace's canonical chat — the canonical session's agent may differ
  from the workspace default. Session identity now remembers its agent
  (`harness` is load-bearing), and the renderer never constructs agent
  argv — the daemon decides per-harness.
- **Resume for every agent.** Per-provider resume playbooks: Claude and
  Grok pre-mint session ids; Pi/Codex/Gemini/Cursor mint their own and K2
  adopts them post-hoc; Codex's `resume <id>` subcommand grammar handled.
  Fixes a real bug: `k2 msg`/`talk` waking a dormant workspace hardcoded
  spawning claude — waking a Grok workspace now resumes Grok.
- **Grok + Hermes session discovery.** Both agents' sessions appear in the
  chat drawers and pickers (Grok: summary.json walk with subagent
  filtering; Hermes: WAL-safe read-only SQLite), proven against live
  session stores. Hermes joins the resume tables; the Hermes tab icon
  renders correctly (its SVG violated the 1em sizing contract).
- **Honest activity signals per agent.** Working/idle detection speaks each
  agent's dialect (verbatim status phrases, title conventions); Grok's
  `⚠ Action Required` title drives the red permission indicator; the
  `k2 talk` HITL fast-path recognizes all studied agents' permission and
  menu dialogs. Injection safety hardened: per-provider readiness timing,
  and K2 will never blind-Enter a Grok permission gate (its default answer
  is "always approve" — a paste there could have permanently yolo-moded
  the session).

## Kessel rendering parity

A run of fixes closing the visual gap with grid-native terminals
(iTerm-class), each independently revertable:

- **Column-anchored rendering.** Every styled run is pinned to its true
  grid column with clipping — fallback-font glyphs (braille art, exotic
  symbols) can no longer push the rest of the row out of alignment
  (Grok's welcome menu misalignment and fragmented box borders).
- **Per-character cells for exotic runs.** Braille/block/box/PUA runs
  render one cell per character, so animated art (Grok's shimmering logo)
  is geometrically frozen — colors change, geometry cannot.
- **Synthetic glyphs.** Box-drawing lines, block elements, shades, and
  sextants are painted as device-pixel-crisp CSS geometry instead of font
  glyphs — TUI borders join seamlessly across cells and stacked block art
  (Claude's logo) loses its horizontal seams. Diagonals and powerline
  glyphs deliberately stay font-rendered.
- **Centered grid + full-width columns.** The sub-cell sizing remainder is
  split evenly between left and right edges (it used to pile up on the
  right), and the right-edge padding now goes to the column math — up to
  one extra column per pane.
- **Background seam fill.** A fullscreen TUI's own background color extends
  into the cell-quantization gutter at the right/bottom edges.

## Also

- Terminal link detection across soft-wrapped rows.
- Stale built-in preset-count test asserts fixed (Grok's addition in
  0.40.21 made 13).
- Feedback F1 groundwork: `k2 feedback` CLI, daemon routes, and data layer
  (agent→human asks; UI lands in a later release).
