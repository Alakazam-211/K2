// How a Home chat bubble's reaction chips come and go, so a reaction never
// jolts the messages around it:
// - a chip that goes (yours taken back, a send that failed and rolled
//   back) stays in its place while it fades and narrows out, not clickable;
// - a first reaction grows the chip row in, pushing what's below down
//   smoothly; the last chip going folds the row away the same way;
// - a row drawn with its chips already there (scrolled into view, a chat
//   opened) just draws.
// All on the fast motion token; under reduced motion chips just appear and
// go.

import { useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react'
import type { HomeChatReaction } from '@/lib/home-chat/api'
import { fastMotionTiming } from '@/lib/motion-timing'

interface Gone {
  r: HomeChatReaction
  /** Where it was in the list. */
  at: number
}

export interface ShownChip {
  r: HomeChatReaction
  leaving: boolean
}

function chipEl(box: HTMLElement | null, emoji: string): HTMLElement | null {
  if (!box) return null
  for (const el of Array.from(box.children)) if (el instanceof HTMLElement && el.getAttribute('data-emoji') === emoji) return el
  return null
}

export function useChipMotion(
  reactions: readonly HomeChatReaction[],
  reduced: boolean,
): { shown: ShownChip[]; boxRef: RefObject<HTMLDivElement | null> } {
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [prev, setPrev] = useState(reactions)
  const [gone, setGone] = useState<readonly Gone[]>([])
  // New reactions: whatever went from the list starts leaving (state from
  // props, set while rendering, so the chip never misses a frame).
  if (prev !== reactions) {
    setPrev(reactions)
    const live = new Set(reactions.map((r) => r.emoji))
    const kept = reduced ? [] : gone.filter((g) => !live.has(g.r.emoji))
    const left = reduced ? [] : prev.flatMap((r, at) => (live.has(r.emoji) || kept.some((g) => g.r.emoji === r.emoji) ? [] : [{ r, at }]))
    if (left.length > 0 || kept.length !== gone.length) setGone([...kept, ...left])
  }
  const shown = useMemo(() => {
    const out: ShownChip[] = reactions.map((r) => ({ r, leaving: false }))
    for (const g of [...gone].sort((a, b) => a.at - b.at)) out.splice(Math.min(g.at, out.length), 0, { r: g.r, leaving: true })
    return out
  }, [reactions, gone])

  const fading = useRef(new Map<string, Animation>())
  const fold = useRef<Animation | null>(null)
  /** Whether the row was drawn last time (null: not drawn yet at all). */
  const drawn = useRef<boolean | null>(null)
  useLayoutEffect(() => {
    const box = boxRef.current
    const timing = fastMotionTiming()
    const drop = (emoji: string): void => {
      fading.current.delete(emoji)
      setGone((g) => g.filter((x) => x.r.emoji !== emoji))
    }
    // Back before it finished leaving (a quick re-react): whole again.
    for (const [emoji, anim] of fading.current) {
      if (gone.some((g) => g.r.emoji === emoji)) continue
      anim.cancel()
      fading.current.delete(emoji)
      const el = chipEl(box, emoji)
      if (el) el.style.overflow = ''
    }
    for (const g of gone) {
      if (fading.current.has(g.r.emoji)) continue
      const el = chipEl(box, g.r.emoji)
      if (!el || typeof el.animate !== 'function') {
        drop(g.r.emoji)
        continue
      }
      el.style.overflow = 'hidden'
      const anim = el.animate([{ opacity: 1, width: `${el.offsetWidth}px` }, { opacity: 0, width: '0px', paddingLeft: '0px', paddingRight: '0px' }], { ...timing, fill: 'forwards' })
      anim.onfinish = () => drop(g.r.emoji)
      fading.current.set(g.r.emoji, anim)
    }
    const was = drawn.current
    drawn.current = box !== null
    if (!box || reduced || typeof box.animate !== 'function') return
    if (reactions.length > 0 && fold.current) {
      fold.current.cancel()
      fold.current = null
    }
    if (was === false && reactions.length > 0) {
      box.animate([{ height: '0px', opacity: 0 }, { height: `${box.offsetHeight}px`, opacity: 1 }], timing)
    } else if (reactions.length === 0 && gone.length > 0 && !fold.current) {
      fold.current = box.animate([{ height: `${box.offsetHeight}px` }, { height: '0px', marginTop: '0px' }], { ...timing, fill: 'forwards' })
    }
  }, [reactions, gone, reduced])
  // Gone with the row: no fold left over for the next one.
  useLayoutEffect(() => {
    if (shown.length === 0) fold.current = null
  }, [shown.length])

  return { shown, boxRef }
}
