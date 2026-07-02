// OSC 52 clipboard-apply policy (client half). The daemon forwards
// every ClipboardStore to every attached viewer; each pane decides
// LOCALLY whether to touch the OS clipboard (the active-viewer check
// lives in TerminalPane — this module is the pure, testable part).

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
