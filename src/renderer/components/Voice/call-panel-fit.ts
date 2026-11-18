// The headset list's call row (VoicePopover's call panel; a DM call uses the
// same panel) holds its round buttons on one line inside the list, with Leave
// call last and always shown in full.
//
// With the native engine, video on and Record shown, the row can hold more
// buttons than the list is wide. Then the lower-priority ones fold, in
// FOLD_ORDER, until the row fits:
//
//   pick   Choose screen (the ▾ beside Share screen). Share screen then
//          opens the list itself where the OS has no picker of its own, as
//          the floating tile's Share screen does; where it has one, the OS
//          picker does the choosing.
//   noise  Noise removal (the wave): the same choice is in Settings → Voice.
//
// Mute, Deafen, Camera, Share screen, Record or Stop and Leave call never
// fold. The row wraps as a last resort (a scrollbar in a short window), so
// Leave call is never cut off at the list's edge.

/** The headset's list (px). */
export const VOICE_POPOVER_WIDTH = 320
/** The list's side padding (`px-3`) and border, each side (px). */
const VOICE_POPOVER_INSET = 12 + 1
/** The call row's width inside the list (px). */
export const CALL_PANEL_ROW_WIDTH = VOICE_POPOVER_WIDTH - 2 * VOICE_POPOVER_INSET
/** The row's gap between buttons (`gap-1.5`, px). */
export const CALL_PANEL_GAP = 6

/** The buttons the call row may show, in their order. */
export type CallPanelButtonId = 'mute' | 'deafen' | 'noise' | 'camera' | 'screen' | 'pick' | 'record' | 'leave'

/** Which buttons fold first when the row is too wide. */
export const FOLD_ORDER: readonly CallPanelButtonId[] = ['pick', 'noise']

/** The width a row of `count` buttons takes (px). */
export function callPanelRowWidth(count: number, button: number): number {
  return count <= 0 ? 0 : count * button + (count - 1) * CALL_PANEL_GAP
}

/** The buttons that fold out of the row so the rest fit `width`. */
export function callPanelFolds(ids: readonly CallPanelButtonId[], button: number, width = CALL_PANEL_ROW_WIDTH): Set<CallPanelButtonId> {
  const folds = new Set<CallPanelButtonId>()
  let count = ids.length
  for (const id of FOLD_ORDER) {
    if (callPanelRowWidth(count, button) <= width) break
    if (!ids.includes(id)) continue
    folds.add(id)
    count -= 1
  }
  return folds
}
