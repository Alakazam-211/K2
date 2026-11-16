// The monitor's details (the ⓘ): everything the picture leaves out, in
// plain words. Picture sizes, timings, file names, codes and the source
// timecode live here and nowhere else on the tab.

import type { TimelineModel } from './fold'
import type { MonitorView } from './monitor-view'
import { dfSrcTc, srcTc } from './picture'
import type { SimState } from './sim'

export type DetailRow = [label: string, value: string]

/** The code a preview host answers when it can't decode a timeline's media (wire shapes, K2 2026-11-15). */
export const NOT_ON_THIS_HOST = 'not_on_this_host'

export function detailRows(S: SimState, M: TimelineModel, v: MonitorView, fileName: string): DetailRow[] {
  const rows: DetailRow[] = []
  const add = (k: string, val: string | null | undefined): void => {
    if (val) rows.push([k, val])
  }
  const pv = v.pv
  const p = v.picture
  add('File', `${fileName}, a timeline file in the edits folder`)
  if (S.tier === 'live') add('Version', `number ${S.B.rev} of the live file`)
  if (M.T.rate && !M.T.provisional) add('Frame rate', `${+M.T.rate.fps.toFixed(3)} frames a second, starting at ${M.tc(0n)}`)
  add('Length', M.dur(M.endF))
  add('Preview', pv.player === 'still' ? (pv.why === 'decode' ? 'stills only: this computer can’t play the preview video' : 'stills only: no player yet') : 'plays the top video track and switches clips at the cuts; no titles, effects or transitions')
  if (pv.player !== 'still') {
    const rung = pv.rung || 1920
    add('Picture size', `${v.state === 'seeking' ? 'a small picture while it finds the frame, then ' : ''}${rung >= 1920 ? 'full size' : 'a smaller size'} (${rung} pixels wide)`)
  }
  if (p?.mi) add('Source file', String(p.mi.file ?? ''))
  if (p?.mi && p.mi.origin === 'unknown') add('Timing', 'from the file’s own timecode')
  if (p?.it?.retimed) add('Speed', 'this clip is retimed in the file; the preview plays it at normal speed')
  const off = M.tracks.filter((t) => t.enabled === false || t.muted)
  if (off.length) add('Tracks', off.map((t) => `${t.label} ${t.enabled === false ? 'off' : 'muted'}`).join(', '))
  if (p && p.kind === 'clip' && p.srcF != null) {
    const still = p as { stillF?: number | null }
    add('Source timecode', p.mi?.dropFrame ? `${dfSrcTc(M, p)} (drop-frame)` : srcTc(M, still.stillF != null && pv.player === 'still' ? still.stillF : p.srcF))
  }
  if (p?.it) add('Clip starts', M.tc(p.it.s))
  const from = (p as { from?: string } | null)?.from
  if (v.paint.how === 'tile') add('Still', from === 'head' ? 'the clip’s first frame' : 'the nearest picture from the clip’s strip (one every 2 s)')
  else if (v.paint.how === 'poster') add('Still', 'the clip’s poster')
  if (p && (p.kind === 'nopreview' || p.kind === 'offline')) add('Why', p.kind === 'offline' ? 'the computer that holds the file can’t reach it' : 'the computer that holds the file can’t read it')
  if (p && p.kind === 'nopreview') add('Code', NOT_ON_THIS_HOST)
  if (S.refused && S.refused.at === S.i) add('Refusal code', S.refused.code)
  if (v.prepState) add('Ready so far', `${v.prepState.n} of ${v.prepState.total} clips`)
  return rows
}
