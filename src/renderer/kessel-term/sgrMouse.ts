// SGR mouse-button forwarding for mouse-reporting TUIs (?1000/?1002
// with ?1006 SGR encoding — claude's fullscreen mode sets all of
// them, plus ?1003 hover which K2 deliberately does NOT forward yet).
// Pure helpers: TerminalPane's forwarding effect owns the DOM events;
// everything decidable/encodable without a DOM lives here so it's
// unit-testable.
//
// Modifier precedence (the convention all three reference emulators
// share — xterm.js MouseService/SelectionService, alacritty
// input/mod.rs, wezterm bypass_mouse_reporting_modifiers; see
// `.k2/notes/tui-mouse-interaction-study.md`):
//   - cmd        → links, handled locally, never forwarded;
//   - shift/option → K2-native local selection (the universal bypass);
//   - plain      → the app's.

export interface MouseGate {
  mouseReport?: boolean
  sgrMouse?: boolean
}

export interface MouseMods {
  shift: boolean
  alt: boolean
  ctrl: boolean
  meta: boolean
}

export type MouseRoute = 'forward' | 'local'

/** Decide whether a button gesture is forwarded to the app or stays
 *  K2-local. SGR-only, same predicate as the wheel branch: legacy X10
 *  byte encoding can't ride the JSON text input channel. */
export function mouseRoute(gate: MouseGate, mods: MouseMods): MouseRoute {
  if (!gate.mouseReport || !gate.sgrMouse) return 'local'
  // cmd = link modifier: handled locally so a link never opens twice
  // (SGR has no meta bit to forward anyway).
  if (mods.meta) return 'local'
  // Shift-drag (and Option-drag, iTerm-style) forces K2's native
  // selection: xterm.js `shouldForceSelection`, alacritty's `!shift`
  // gate, wezterm's SHIFT bypass default.
  if (mods.shift || mods.alt) return 'local'
  return 'forward'
}

/** MouseEvent.button → SGR base button code (left 0 / middle 1 /
 *  right 2). Exotic buttons (back/forward) collapse to left rather
 *  than emitting codes ?1000-era TUIs can't interpret. */
export function sgrButtonCode(button: number): number {
  return button === 1 || button === 2 ? button : 0
}

export type SgrKind = 'press' | 'motion' | 'release'

/** Encode one SGR mouse report. Motion adds +32; ctrl adds +16.
 *  Shift (+4) and alt (+8) are deliberately NEVER encoded: they are
 *  K2's local-selection overrides, so a forwarded gesture can't carry
 *  them at press time, and xterm.js strips its override modifier from
 *  forwarded reports for the same reason (the app must not see a
 *  phantom modifier mid-drag). Release uses the `m` final WITH the
 *  real button code — expressible only in SGR (legacy X10 release is
 *  always button 3), which is why the gate requires ?1006. */
export function encodeSgrMouse(
  button: number,
  kind: SgrKind,
  ctrl: boolean,
  col: number,
  row: number,
): string {
  let code = sgrButtonCode(button)
  if (kind === 'motion') code += 32
  if (ctrl) code += 16
  return `\x1b[<${code};${col};${row}${kind === 'release' ? 'm' : 'M'}`
}

export interface Cell {
  col: number
  row: number
}

/** Drag-motion cell gate (alacritty input/mod.rs:513): report motion
 *  only when the pointer crossed into a different CELL. Collapses a
 *  pixel-granular drag into ≤ cols+rows reports before the token
 *  bucket even sees it. */
export function cellChanged(prev: Cell | null, next: Cell): boolean {
  return prev === null || prev.col !== next.col || prev.row !== next.row
}
