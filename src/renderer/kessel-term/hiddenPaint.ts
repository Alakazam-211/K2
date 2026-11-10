// Pausing a hidden grid's drawing (prd-projects-threads-v1, slice P4).
//
// A Projects tile in Thread, Chatter or Heartbeats view keeps its
// TerminalPane mounted with the grid in a display:none column. The grid
// socket stays open and every frame still merges into the pane's live grid
// (`liveGridRef`) and is acked, so the session, its size and the daemon's
// flow control never notice. What stops is the per-frame React commit of
// the rendered `snapshot`, which re-rendered the whole pane (and the Thread
// beside it) for a grid nobody can see. On show, the pane commits the live
// grid in a layout effect, so the first frame painted is the current one.
// An older server's busy-footer scan (TerminalPane's legacy-activity path)
// still reads every paused frame.
//
// Client-only: nothing here closes the socket, sends the daemon anything,
// or drops a frame.

/** How long the grid must stay hidden before drawing pauses. A quick peek
 *  at another view and back never pauses at all. */
export const HIDDEN_PAINT_PAUSE_MS = 2000

/** Whether a pane should stop drawing its grid (once the delay has run).
 *  - `enabled`: the caller opted in (Projects tiles only).
 *  - `terminalOnScreen`: the grid's column is laid out (Terminal view, a
 *    Split with a Terminal side, or Thread view's revealed terminal). */
export function shouldPauseHiddenPaint(opts: {
  enabled: boolean
  terminalOnScreen: boolean
}): boolean {
  return opts.enabled && !opts.terminalOnScreen
}
