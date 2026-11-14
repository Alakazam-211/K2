// Home groups: what the sideways swipe looks like in the nav (K2 Design's
// mock, Rosson's call: slide plus peek, and no reduced-motion variant).
//   - While two fingers move, the agent list follows by up to 40% of its
//     width and fades towards half, and the group it would land on peeks in
//     from that side: its glyph and name, "Next group" (or "Previous
//     group"), then "Let go to switch" past SWIPE_COMMIT_AT with a stronger
//     edge. The rail peeks the glyph alone.
//   - Let go past the line: one short move, out then in, on the fast motion
//     token each (100 ms Terminal, 140 ms Glass), ease-out, no bounce. The
//     old group's rows slide out, the new group's slide in.
//   - Let go short of it: the list slides back on the same token.
// The main pane never moves. The logic (thresholds, commit) is
// `lib/group-swipe.ts` and `useGroupSwipe`; this only reads its store.

import { useEffect, useRef, useState } from 'react'
import { SWIPE_COMMIT_AT } from '@/lib/group-swipe'
import { useGroupSwipeStore } from '@/hooks/useGroupSwipe'
import { useHomesStore } from '@/stores/homes'
import { GroupGlyph } from './GroupGlyph'
import { SWIPE_FOLLOW, SWIPE_PEEK_FULL_AT } from './group-bar-look'

/** The settle's two halves start and end here (share of the list's width). */
const OUT_TO = 0.6
const IN_FROM = 0.24

/** The Style's fast motion token, in ms (100 Terminal, 140 Glass). */
export function fastMotionMs(): number {
  if (typeof document === 'undefined') return 120
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--motion-duration-fast').trim()
  const n = parseFloat(raw)
  if (!Number.isFinite(n)) return 120
  return raw.endsWith('ms') ? n : n * 1000
}

const EASE = 'var(--motion-duration-fast) var(--motion-ease-out-soft, ease-out)'

type Phase = { kind: 'out'; fromId: string; sign: number } | { kind: 'in'; sign: number } | null

export interface GroupSwipeSlide {
  /** The style for the list's content (it slides; the frame stays). */
  listStyle: React.CSSProperties
  /** During the first half of a committed settle: the group whose rows
   *  slide out (the one just left). Null otherwise. */
  shownGroupId: string | null
  /** The peek, while a swipe is under way and there is a group that way. */
  peek: React.JSX.Element | null
}

export function useGroupSwipeSlide(variant: 'nav' | 'rail'): GroupSwipeSlide {
  const swipe = useGroupSwipeStore((s) => s.swipe)
  const release = useGroupSwipeStore((s) => s.release)
  const homes = useHomesStore((s) => s.homes)
  const selectedId = useHomesStore((s) => s.selectedId)
  const [phase, setPhase] = useState<Phase>(null)
  // The group a swipe started from (it doesn't change until the release).
  const fromRef = useRef<string | null>(null)
  if (swipe) fromRef.current = selectedId
  const seenSeq = useRef(release?.seq ?? 0)

  useEffect(() => {
    if (!release || release.seq === seenSeq.current) return
    seenSeq.current = release.seq
    if (release.outcome !== 'commit' || !fromRef.current) return
    const sign = release.direction === 'next' ? 1 : -1
    const fast = fastMotionMs()
    setPhase({ kind: 'out', fromId: fromRef.current, sign })
    let raf = 0
    const t = setTimeout(() => {
      setPhase({ kind: 'in', sign })
      raf = requestAnimationFrame(() => {
        raf = requestAnimationFrame(() => setPhase(null))
      })
    }, fast)
    return () => {
      clearTimeout(t)
      cancelAnimationFrame(raf)
    }
  }, [release])

  let listStyle: React.CSSProperties
  if (phase?.kind === 'out') {
    listStyle = { transform: `translateX(${-phase.sign * OUT_TO * 100}%)`, opacity: 0, transition: `transform ${EASE}, opacity ${EASE}` }
  } else if (phase?.kind === 'in') {
    listStyle = { transform: `translateX(${phase.sign * IN_FROM * 100}%)`, opacity: 0, transition: 'none' }
  } else if (swipe && swipe.progress > 0) {
    const sign = swipe.direction === 'next' ? 1 : -1
    listStyle = {
      transform: `translateX(${(-sign * swipe.progress * SWIPE_FOLLOW * 100).toFixed(2)}%)`,
      opacity: 1 - swipe.progress * 0.5,
      transition: 'none',
    }
  } else {
    listStyle = { transform: 'translateX(0)', opacity: 1, transition: `transform ${EASE}, opacity ${EASE}` }
  }

  let peek: React.JSX.Element | null = null
  if (swipe && swipe.progress > 0 && !phase) {
    const at = homes.findIndex((h) => h.id === selectedId)
    const sign = swipe.direction === 'next' ? 1 : -1
    const to = at + sign
    const target = at >= 0 && to >= 0 && to < homes.length ? homes[to] : null
    if (target) {
      const armed = swipe.progress >= SWIPE_COMMIT_AT
      const shift = (1 - Math.min(swipe.progress / SWIPE_PEEK_FULL_AT, 1)) * 110 * sign
      const side = variant === 'rail' ? { [sign > 0 ? 'right' : 'left']: 2 } : { [sign > 0 ? 'right' : 'left']: 8 }
      peek = (
        <div
          data-group-float="peek"
          data-group-swipe-peek={target.id}
          data-armed={armed ? '' : undefined}
          role={variant === 'rail' ? undefined : 'status'}
          aria-hidden={variant === 'rail' ? 'true' : undefined}
          className={`pointer-events-none absolute top-[38%] z-20 flex flex-col gap-0.5 ${variant === 'rail' ? 'p-1.5' : 'px-3 py-2'}`}
          style={{ ...side, transform: `translateX(${shift.toFixed(1)}%)` }}
        >
          {variant === 'rail' ? (
            <span className="text-[var(--color-text-secondary)]">
              <GroupGlyph icon={target.icon ?? null} index={to} />
            </span>
          ) : (
            <>
              <span className="flex items-center gap-2 whitespace-nowrap text-[13px] font-semibold text-[var(--color-text-primary)]">
                <span className="text-[var(--color-text-secondary)]">
                  <GroupGlyph icon={target.icon ?? null} index={to} />
                </span>
                {target.name}
              </span>
              <span className="whitespace-nowrap text-[10px] text-[var(--color-text-secondary)]">
                {armed ? 'Let go to switch' : sign > 0 ? 'Next group' : 'Previous group'}
              </span>
            </>
          )}
        </div>
      )
    }
  }

  return { listStyle, shownGroupId: phase?.kind === 'out' ? phase.fromId : null, peek }
}
