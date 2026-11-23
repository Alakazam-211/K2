// The open activity ear against a fake layout (jsdom lays nothing out):
// one turn's frames the way the daemon sends them during a burst of tool
// calls, and the ear's height as a browser would stack its lines. Shared by
// ZenActivityDrawer.calm.test.tsx and ThreadEarCalm.test.tsx.
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { ZEN_DRAWER_NOTE_LINES, ZEN_DRAWER_STEP_LINES } from '@/components/Zen/widgets/ZenActivityDrawer'

export const T0 = Date.now()

/** One frame of turn `id` (rev `rev`): `phase`, the tool `line`, the tally,
 *  and the children (a reply already sent). */
export function frame(rev: number, phase: string, line: string | null, extra: Record<string, unknown> = {}, id = 'calm-1'): ThreadTurn {
  const t = coerceThreadTurn(
    {
      turnId: id,
      startedAt: T0 - 30_000,
      state: 'working',
      phase,
      line,
      subagents: 0,
      background: 0,
      tally: { read: 1 },
      end: null,
      rev,
      serverNow: T0,
      ...extra,
    },
    T0,
  )
  if (!t) throw new Error(`fixture is not a turn: ${phase}`)
  return t
}

export const LONG = `Running \`${'cargo test -p k2-daemon --features test-util -- thread_activity '.repeat(3)}\``

/** A tool call's run of frames, the way the daemon sends them: each call as
 *  it starts and ends, thinking between, a reply, the tally growing. */
export function burst(): ThreadTurn[] {
  return [
    frame(2, 'tool', 'Reading `src/a.ts`', { tally: { read: 2 } }),
    frame(3, 'thinking', null, { tally: { read: 2 } }),
    frame(4, 'tool', LONG, { tally: { read: 2, cmd: 1 } }),
    frame(5, 'thinking', null, { tally: { read: 2, cmd: 1 } }),
    frame(6, 'tool', 'Searching `useCoalescedTurn`', { tally: { read: 2, cmd: 1, search: 1 } }),
    frame(7, 'children', null, {
      replied: true,
      background: 1,
      children: [{ kind: 'monitor', state: 'running', label: 'the release build', since: T0 - 5_000 }],
    }),
    frame(8, 'thinking', null, { tally: { read: 3, cmd: 1, search: 1 } }),
    frame(9, 'tool', 'Editing `ZenActivityDrawer.tsx`', { tally: { read: 3, cmd: 1, search: 1, edit: 1 } }),
    frame(10, 'thinking', null, { tally: { read: 3, cmd: 1, search: 1, edit: 1 } }),
  ]
}

/** How tall the open ear's words stack (px), as a browser lays them out at
 *  this test's width: 6px padding each side, 18px head, 19px step lines,
 *  18px tally and subagent lines, 1px between; each clamped to its cap. */
export function naturalEarPx(detail: HTMLElement): number {
  const lines = (el: Element | null, perLine: number, cap: number): number => {
    if (!el) return 0
    const n = Math.max(1, Math.ceil((el.textContent ?? '').length / 40))
    return Math.min(n, cap) * perLine + 1
  }
  return (
    12 +
    18 +
    lines(detail.querySelector('[data-zen-drawer-step-text]'), 19, ZEN_DRAWER_STEP_LINES) +
    lines(detail.querySelector('[data-zen-drawer-tally]'), 18, ZEN_DRAWER_NOTE_LINES) +
    lines(detail.querySelector('[data-zen-drawer-children]'), 18, ZEN_DRAWER_NOTE_LINES)
  )
}

/** The open ear's laid-out height: its words, or its min-height if taller. */
export function earRectPx(detail: HTMLElement): number {
  return Math.max(naturalEarPx(detail), parseFloat(detail.style.minHeight) || 0)
}
