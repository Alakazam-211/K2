// The monitor when there's no edit to show (T6), pure, in the mock's words:
//   - opening: each part of the file as it arrives (connecting, who you are
//     and the lanes, then the timeline's parts), "Step N of M"; the lanes
//     fill in under it, the picture comes once the clips are in;
//   - no clips yet, with Ask <agent>;
//   - a file that can't be watched live (the kit's refusal at open), with
//     why, Open saved versions and Ask <agent>.
// Actions are the shell's notice actions, so one handler answers both.

import type { NoticeActionId } from '../shared/notices'
import type { TimelineModel } from './fold'
import type { SimState } from './sim'

export interface CardAction {
  id: Extract<NoticeActionId, 'open-saved' | 'ask'>
  label: string
  primary?: true
}

export type StageCard =
  | { kind: 'opening'; title: string; line: string; progress: number; rows: { text: string; state: 'done' | 'now' | 'wait'; n: number | null }[] }
  | { kind: 'empty'; title: string; line: string; actions: CardAction[] }
  | { kind: 'declined'; title: string; line: string; actions: CardAction[] }

/** What the timeline's parts are called while they arrive. */
const PART: Record<string, string> = {
  timeline: 'The timeline’s rate and start',
  tracks: 'Tracks',
  track_props: 'Track names',
  items: 'Clips',
  item_props: 'Clip details',
  markers: 'Markers',
  marker_props: 'Marker names',
  media: 'Media',
  media_props: 'Media details',
}

/** The card in the monitor's place, or null when there's an edit to show. */
export function stageCard(S: SimState, M: TimelineModel, o: { agent: string; file: string }): StageCard | null {
  const ask: CardAction = { id: 'ask', label: `Ask ${o.agent}` }
  const ld = S.loading
  if (ld) {
    const n = ld.roots.length + 2
    const done = ld.phase === 'units' ? ld.roots.length : ld.phase === 'pages' ? ld.upto + 1 : 0
    const stepNo = Math.min(n, done + (ld.phase === 'connect' ? 1 : ld.phase === 'welcome' ? 2 : 3))
    const progress = Math.round((100 * (done + (ld.phase === 'connect' ? 0 : 1) + (ld.phase === 'units' ? 1 : 0))) / n)
    const head: StageCard & { kind: 'opening' } = {
      kind: 'opening',
      title: `Opening ${o.file}`,
      line: `The lanes fill in as each part arrives; the picture comes once the clips are in. Step ${stepNo} of ${n}.`,
      progress,
      rows: [
        { text: 'Connecting', state: ld.phase !== 'connect' ? 'done' : 'now', n: null },
        { text: 'Who you are and the lanes', state: ld.phase !== 'connect' && ld.phase !== 'welcome' ? 'done' : ld.phase === 'welcome' ? 'now' : 'wait', n: null },
        ...ld.roots.map((r) => {
          const d = done > r.i
          return { text: PART[r.name] ?? r.name, state: d ? ('done' as const) : ld.phase === 'pages' && ld.upto === r.i ? ('now' as const) : ('wait' as const), n: d && r.n ? r.n : null }
        }),
      ],
    }
    return head
  }
  if (S.tier === 'declined' && S.decline) return { kind: 'declined', title: S.decline.title, line: S.decline.text, actions: [{ id: 'open-saved', label: 'Open saved versions', primary: true }, ask] }
  if (S.tier === 'live' && !M.count) return { kind: 'empty', title: 'No clips on this timeline yet', line: `When ${o.agent} adds clips, the picture shows here.`, actions: [ask] }
  return null
}
