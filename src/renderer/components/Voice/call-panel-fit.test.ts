// The headset list's call row fits the list: Leave call is last and always
// shown in full, in every call (audio only, video on, recording, sharing).

import { describe, expect, it } from 'vitest'
import { CALL_PANEL_BUTTON } from './CallControls'
import { CALL_PANEL_ROW_WIDTH, VOICE_POPOVER_WIDTH, callPanelFolds, callPanelRowWidth, type CallPanelButtonId } from './call-panel-fit'

/** The row's buttons for one kind of call, in the panel's order. */
function row(o: { native: boolean; video: boolean; list: boolean; sharing: boolean; record: boolean }): CallPanelButtonId[] {
  const ids: CallPanelButtonId[] = ['mute', 'deafen']
  if (o.native) ids.push('noise')
  if (o.video) {
    ids.push('camera', 'screen')
    if (o.list && !o.sharing) ids.push('pick')
  }
  if (o.native && o.record) ids.push('record')
  ids.push('leave')
  return ids
}

const bools = [false, true]

describe('call-panel-fit', () => {
  it('the row is the list less its padding and border', () => {
    expect(VOICE_POPOVER_WIDTH).toBe(320)
    expect(CALL_PANEL_ROW_WIDTH).toBe(294)
  })

  it('every call fits the list with Leave call last and shown', () => {
    for (const native of bools)
      for (const video of bools)
        for (const list of bools)
          for (const sharing of bools)
            for (const record of bools) {
              const ids = row({ native, video, list, sharing, record })
              const folds = callPanelFolds(ids, CALL_PANEL_BUTTON)
              const shown = ids.filter((id) => !folds.has(id))
              const what = JSON.stringify({ native, video, list, sharing, record })
              expect(callPanelRowWidth(shown.length, CALL_PANEL_BUTTON), what).toBeLessThanOrEqual(CALL_PANEL_ROW_WIDTH)
              expect(shown.at(-1), what).toBe('leave')
              for (const keep of ['mute', 'deafen', 'camera', 'screen', 'record', 'leave'] as const) {
                if (ids.includes(keep)) expect(shown, what).toContain(keep)
              }
            }
  })

  it('with video on and Record shown, only Choose screen folds; nothing folds when it all fits', () => {
    const full = row({ native: true, video: true, list: true, sharing: false, record: true })
    expect(full).toHaveLength(8)
    expect([...callPanelFolds(full, CALL_PANEL_BUTTON)]).toEqual(['pick'])
    // Sharing: no chevron, so nothing folds.
    expect(callPanelFolds(row({ native: true, video: true, list: true, sharing: true, record: true }), CALL_PANEL_BUTTON).size).toBe(0)
    expect(callPanelFolds(row({ native: true, video: false, list: false, sharing: false, record: true }), CALL_PANEL_BUTTON).size).toBe(0)
  })

  it('a narrower row folds noise removal next', () => {
    const full = row({ native: true, video: true, list: true, sharing: false, record: true })
    expect([...callPanelFolds(full, CALL_PANEL_BUTTON, callPanelRowWidth(6, CALL_PANEL_BUTTON))]).toEqual(['pick', 'noise'])
  })
})
