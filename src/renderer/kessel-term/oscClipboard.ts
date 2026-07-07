// OSC 52 clipboard-apply policy (client half). The daemon TARGETS the
// `clipboard` frame at the ACTIVE subscriber's connection (the one
// whose input made the selection — its Input handler flips
// `active_subscriber` on every frame, including the SGR mouse drag
// that produced the selection), falling back to every subscriber only
// when no claim exists. Each pane still decides LOCALLY whether to
// touch the OS clipboard — this module is the pure, testable part.
//
// The local gate is DOM truth (visible + pane-focused + window-focused
// — `computeDesiredActive`), deliberately NOT the pane's own claim
// mirror (`lastSentActiveRef`): that mirror diverges from the daemon's
// input-flipped `active_subscriber` by design (claim suppression on
// ambient recomputes, release sent off a stale `paneFocusedRef` during
// the first forwarded mousedown, null reset on every WS reconnect), and
// gating the copy on it stranded remote viewers' selections. The DOM
// gate also keeps a pre-targeting daemon's broadcast-to-all from
// stomping passive viewers' clipboards.

import { computeDesiredActive } from './activeViewer'

/** Whether an incoming OSC 52 payload should be written to the OS
 *  clipboard. Empty payloads are refused (a TUI clearing its own
 *  selection must not blank the user's clipboard) and consecutive
 *  identical payloads are refused — claude re-emits the same OSC 52
 *  on every repaint while a selection stays live (5× per selection
 *  observed in the study). A DIFFERENT payload always applies,
 *  including one seen earlier (A→B→A is three real copies). */
export function shouldApplyOsc52(
  lastApplied: string | null,
  incoming: string,
): boolean {
  if (incoming.length === 0) return false
  return incoming !== lastApplied
}

/** Inputs for {@link shouldApplyOsc52Frame} — the full apply decision
 *  for one incoming `clipboard` WS frame. The three visibility booleans
 *  are the pane's DOM-truth state at frame-arrival time (the same trio
 *  `computeDesiredActive` arbitrates for the active-viewer claim). */
export interface Osc52FrameInputs {
  /** This pane's enclosing tab + pane-item are visible (not display:none). */
  visible: boolean
  /** This pane holds shadow-input focus (only one pane does at a time). */
  paneFocused: boolean
  /** The OS window is focused. */
  windowFocused: boolean
  /** Last payload this pane actually wrote to the OS clipboard. */
  lastApplied: string | null
  /** The incoming frame's decoded payload. */
  incoming: string
}

/** Full apply decision for an incoming OSC 52 `clipboard` frame: the
 *  pane must be the one the user is actually looking at and interacting
 *  with (visible, pane-focused, window-focused — the selection that
 *  produced the copy happened moments ago in THIS pane, so all three
 *  hold at frame arrival), and the payload must pass the empty/dedupe
 *  policy of {@link shouldApplyOsc52}. */
export function shouldApplyOsc52Frame(inputs: Osc52FrameInputs): boolean {
  if (
    !computeDesiredActive({
      visible: inputs.visible,
      paneFocused: inputs.paneFocused,
      windowFocused: inputs.windowFocused,
    })
  ) {
    return false
  }
  return shouldApplyOsc52(inputs.lastApplied, inputs.incoming)
}
