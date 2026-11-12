// What the view shows, worked out from the state and the person's own
// gestures: which sheet is on stage, whose cell the formula bar describes,
// and the follow rule. Pure.

import { agentFocus, agentId, caretOn, type LiveWorkbookState } from './reducer'
import { sheetsOf } from './workbook'

/** The person's own view state (never sent): a sheet they opened, a cell they clicked. */
export interface LocalView {
  /** A sheet the person picked (a tab, the hidden menu, a history line). */
  pinned: string | null
  /** A cell the person clicked. */
  caret: { sheet: string; r: number; c: number } | null
}

export const EMPTY_LOCAL: LocalView = { pinned: null, caret: null }

/** Following the agent: on by default, only while the doc is live and loaded. */
export function isFollowing(s: LiveWorkbookState): boolean {
  return s.tier === 'live' && !!s.book && !s.building && s.follow && !!agentId(s)
}

/** The sheet on stage: the agent's while following; else the one you picked; else your focus; else the first visible tab. */
export function stageSheet(s: LiveWorkbookState, local: LocalView): string | null {
  if (!s.book) return null
  const all = sheetsOf(s.book)
  const visible = all.filter((x) => !x.hidden).map((x) => x.key)
  if (isFollowing(s)) {
    const af = agentFocus(s)
    if (af && visible.includes(af)) return af
  }
  if (local.pinned && all.some((x) => x.key === local.pinned)) return local.pinned
  const self = s.welcome?.self
  const mine = self ? s.peers[self]?.focus?.key : null
  if (mine && visible.includes(mine)) return mine
  return visible[0] ?? all[0]?.key ?? null
}

/** The cell the formula bar describes: yours if you clicked one; the agent's caret while following; else your caret; else the agent's. */
export function formulaTarget(s: LiveWorkbookState, local: LocalView, sheet: string): { r: number; c: number; who: 'you' | 'agent' } | null {
  if (!s.book) return null
  if (local.caret && local.caret.sheet === sheet) return { r: local.caret.r, c: local.caret.c, who: 'you' }
  const self = s.welcome?.self
  const me = self ? s.peers[self] : undefined
  const aId = agentId(s)
  const ag = aId ? s.peers[aId] : undefined
  if (isFollowing(s)) {
    const rc = caretOn(s.book, ag, sheet)
    if (rc) return { ...rc, who: 'agent' }
  }
  const mine = caretOn(s.book, me, sheet)
  if (mine) return { ...mine, who: 'you' }
  const rc = caretOn(s.book, ag, sheet)
  return rc ? { ...rc, who: 'agent' } : null
}
