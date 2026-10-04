// Home keyboard chords (0.43.2).
//
// Two chords, one window-level owner (`hooks/useWorkspaceIndexShortcuts`):
//
//   - Home rows: Cmd+1–9 selects Home row N (Home M3). Like the Agents
//     pinned area, it is `metaKey` on every platform and paints as `⌘ 1-9`
//     beside a 1–9 position badge on each row.
//   - Home switcher: HOME_SWITCH_BINDING below picks the Nth named Home
//     (`k2.homes.v1` order, the order of the Home dropdown).
//
// Bindings the switcher must stay clear of:
//   - Ctrl+1–9 (Meta, Alt, Shift up) — launch preset N (useTerminalShortcuts).
//   - Cmd+1–9 / Cmd+0 — Home row N on Home; pinned/active workspace elsewhere.
//   - Cmd+Option+1–9 OFF Home — the Agents workspace switch (pinned or
//     active, per Settings → shortcut layout). Same chord as the switcher,
//     scoped by page: on Home it switches Homes, anywhere else it does what
//     it always did. Never both.
//   - Cmd+1–9 on Projects — pane N (`paneSwitchDigit`).

/** The modifier state of a keydown, plus what is needed to read its digit. */
export interface ChordEvent {
  code: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
}

/**
 * The Home switcher: Cmd+Option+1–9 (⌥⌘N) selects the Nth named Home, on
 * the Home page only. It is the Agents page's own workspace-switch chord
 * (Rosson 2026-10-04: "like we did on the agents page"), with the same
 * modifiers on every platform (`metaKey` + `altKey`, as that handler reads
 * them) and the same `⌥⌘` glyphs.
 *
 * Rosson first asked for Cmd+Shift+1–9. It is NOT the default: macOS keeps
 * Cmd+Shift+3, 4 and 5 (and 6 on Touch Bar Macs) for screenshots. The app
 * never receives those keys, and taking them would break screenshots.
 */
export const HOME_SWITCH_BINDING = {
  metaKey: true,
  ctrlKey: false,
  altKey: true,
  shiftKey: false,
  /** Prefix painted before the digit (the Agents pinned header's glyphs). */
  label: '⌥⌘',
} as const

/** How many Homes the switcher reaches (1–9). */
export const HOME_SWITCH_LIMIT = 9

/** The digit (1–9) when a keydown is EXACTLY the Home switcher chord, else
 *  null. Reads `e.code`, since Option changes `e.key` on macOS. Page scope
 *  is the caller's: this is also the Agents chord off Home. */
export function homeSwitchDigit(e: ChordEvent): number | null {
  const b = HOME_SWITCH_BINDING
  if (
    e.metaKey !== b.metaKey ||
    e.ctrlKey !== b.ctrlKey ||
    e.altKey !== b.altKey ||
    e.shiftKey !== b.shiftKey
  ) {
    return null
  }
  const m = /^Digit([1-9])$/.exec(e.code)
  if (!m) return null
  const n = Number(m[1])
  return n <= HOME_SWITCH_LIMIT ? n : null
}

/** The chord as painted next to Home `n` in the Home dropdown. */
export function homeSwitchCombo(n: number): string {
  return `${HOME_SWITCH_BINDING.label}${n}`
}

/** The Home rows' chord hint (beside the Home's row count), matching the
 *  Agents pinned header. `metaKey` on every platform, like that header. */
export const HOME_ROW_SHORTCUT_HINT = '⌘ 1-9'
