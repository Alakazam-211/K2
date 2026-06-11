// Kessel — configuration surface.
//
// Consolidates every knob that used to live as a hardcoded constant
// (font stack, scrollback cap, settle ms, wheel multiplier, bell
// duration, sync-update timeout, default colors) into a single
// KesselConfig type with sensible defaults.
//
// The shape is deliberately alacritty-flavored so users who come
// from alacritty.yml can map their settings one-to-one. Fields
// not yet honored by a Kessel deliverable are still present in
// the type — marked with `// TODO(Dn)` so the next deliverable
// can just populate them.
//
// Consumption:
//   - SessionStreamView reads via `useKesselConfig()` (React context).
//   - TerminalGrid and other non-React modules take `KesselConfig`
//     as a constructor option.
//   - Overrides are merged via `mergeKesselConfig(partial)` — deep
//     merge, so callers only specify what they're changing.
//
// Backwards-compat invariant: `defaultKesselConfig` MUST produce
// behavior bit-for-bit identical to pre-config Kessel. Any field
// whose default differs is a product bug.

// ── Public types ────────────────────────────────────────────────────

export interface KesselConfig {
  font: KesselFontConfig
  colors: KesselColorsConfig
  scrolling: KesselScrollingConfig
  cursor: KesselCursorConfig
  bell: KesselBellConfig
  mouse: KesselMouseConfig
  performance: KesselPerformanceConfig
}

export interface KesselFontConfig {
  /** CSS font-family list. First match wins; downstream monospace
   *  fallbacks guarantee legibility when the preferred font isn't
   *  installed. */
  family: string
  /** Size in CSS px. */
  size: number
  /** Line height as a multiplier of `size` — 1.2 feels right for
   *  MesloLGM; 1.0 for denser grids; 1.4+ for accessibility. */
  lineHeightMultiplier: number
  /** Per-glyph render offset. Used later when a custom font needs
   *  vertical/horizontal tuning. Zero by default. */
  offset: { x: number; y: number }
}

export interface KesselColorsConfig {
  /** Default text color when no SGR override. 0xRRGGBB. */
  foreground: number
  /** Default pane background. */
  background: number
  /** 16-color ANSI palette — indices 0..15. Populated by D10 (OSC 4)
   *  at runtime from TUI theme directives; this field is the
   *  compile-time fallback. */
  palette: readonly [
    number, number, number, number,
    number, number, number, number,
    number, number, number, number,
    number, number, number, number,
  ]
  /** Cursor colors. `text` null = use cell fg; `cursor` is the
   *  caret's background. */
  cursor: { text: number | null; cursor: number }
  /** Selection colors. `text` null = use cell fg. */
  selection: { text: number | null; background: number }
}

export interface KesselScrollingConfig {
  /** Max scrollback lines before the oldest is discarded. Matches
   *  TerminalGrid's DEFAULT_SCROLLBACK_CAP. */
  cap: number
  /** Trackpad/wheel scroll sensitivity, unitless. The handler
   *  accumulates pixel deltas and converts them to lines at
   *  `1 line per cell height` (i.e. physical-distance scrolling,
   *  matching every native macOS text view). This value then
   *  scales the line count up or down:
   *    1.0 → Alacritty-equivalent (default).
   *    2.0 → twice as fast.
   *    0.5 → half as fast.
   *  NOT "lines per wheel event" — trackpads send 30-60 events
   *  per swipe, so that semantic would overshoot wildly. */
  multiplier: number
}

export type KesselCursorShape =
  | 'steady_block'
  | 'blinking_block'
  | 'steady_underscore'
  | 'blinking_underscore'
  | 'steady_bar'
  | 'blinking_bar'

export interface KesselCursorConfig {
  /** Shape when the TUI hasn't issued DECSCUSR. 'steady_block'
   *  matches pre-config behavior. Vim will override dynamically
   *  via D13. */
  defaultShape: KesselCursorShape
  /** Resting-position settle window in ms. See SessionStreamView's
   *  resting-cursor effect. Below the 100ms perception floor. */
  settleMs: number
  /** Blink phase length in ms for the DECSCUSR blinking variants.
   *  Only applies when the TUI explicitly requests a blinking shape.
   *  530ms matches the xterm default. */
  blinkIntervalMs: number
  /** Cursor bar/underscore thickness as a fraction of cell height. */
  thickness: number
  /** If true, the cursor renders hollow when the pane is unfocused.
   *  Helps multi-pane layouts signal which pane is active. */
  unfocusedHollow: boolean
}

export type KesselBellMode = 'off' | 'visual' | 'audio' | 'both'

export interface KesselBellConfig {
  /** How to surface BEL. `visual` = background flash; `audio` =
   *  system beep (when available). Default visual. */
  mode: KesselBellMode
  /** Visual flash duration in ms. */
  durationMs: number
  /** Flash color (0xRRGGBB). */
  color: number
}

export interface KesselMouseConfig {
  /** Hide mouse cursor while the user is typing. Alacritty default
   *  is true; we start off so the pointer stays visible for
   *  debugging. */
  hideWhenTyping: boolean
  /** Shift+click/drag/wheel stays local even when the TUI has
   *  mouse mode on. iTerm2 / Terminal.app convention. Leave true
   *  unless the user deliberately disables the escape hatch. */
  shiftOverrideEnabled: boolean
}

export interface KesselPerformanceConfig {
  /** DECSET ?2026 pending-buffer watchdog. If a TUI opens a sync
   *  update and never closes it, we force-flush after this many
   *  ms so the pane can't wedge. 150ms matches alacritty. */
  syncUpdateTimeoutMs: number
  /** Batch WS-delivered Frame events by animation frame instead of
   *  dispatching one-at-a-time. D4. Default true. */
  frameBatchingEnabled: boolean
}

// ── Default config ──────────────────────────────────────────────────

/** The default configuration. Preserves pre-config behavior exactly.
 *  When adding new knobs, set defaults to whatever the prior
 *  hardcoded value was — never change defaults as part of adding
 *  a field. */
export const defaultKesselConfig: KesselConfig = {
  font: {
    family:
      "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
    size: 14,
    lineHeightMultiplier: 1.2,
    offset: { x: 0, y: 0 },
  },
  colors: {
    foreground: 0xe0e0e0,
    background: 0x0a0a0a,
    // Tango palette — indices match ANSI 0..15 (black..bright white).
    // D10 will override this at runtime from OSC 4 directives, so
    // treat these as the "cold-start" defaults before any TUI has
    // declared its theme.
    palette: [
      0x000000, 0xcc0000, 0x4e9a06, 0xc4a000,
      0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf,
      0x555753, 0xef2929, 0x8ae234, 0xfce94f,
      0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
    ],
    cursor: { text: null, cursor: 0xe0e0e0 },
    selection: { text: null, background: 0x444444 },
  },
  scrolling: {
    cap: 10_000,
    multiplier: 1,
  },
  cursor: {
    defaultShape: 'steady_block',
    settleMs: 60,
    blinkIntervalMs: 530,
    thickness: 0.15,
    unfocusedHollow: true,
  },
  bell: {
    mode: 'visual',
    durationMs: 150,
    color: 0xffffff,
  },
  mouse: {
    hideWhenTyping: false,
    shiftOverrideEnabled: true,
  },
  performance: {
    syncUpdateTimeoutMs: 150,
    frameBatchingEnabled: true,
  },
}

// ── Overrides + merge helper ────────────────────────────────────────

/** Deep-partial of KesselConfig — every nested field is optional,
 *  so callers can override just the knobs they care about. */
export type KesselConfigOverrides = {
  [K in keyof KesselConfig]?: Partial<KesselConfig[K]>
}

/** Merge a user override on top of `defaultKesselConfig`. Shallow-
 *  deep-merge: only top-level keys are deep-merged; nested objects
 *  replace wholesale if the caller specifies them. (A full recursive
 *  merge isn't needed yet and keeps the type surface predictable.) */
export function mergeKesselConfig(
  overrides?: KesselConfigOverrides,
): KesselConfig {
  if (!overrides) return defaultKesselConfig
  const out = { ...defaultKesselConfig }
  for (const key of Object.keys(overrides) as (keyof KesselConfig)[]) {
    const patch = overrides[key]
    if (patch === undefined) continue
    // @ts-expect-error — heterogeneous nested merge; runtime-correct
    out[key] = { ...defaultKesselConfig[key], ...patch }
  }
  return out
}
