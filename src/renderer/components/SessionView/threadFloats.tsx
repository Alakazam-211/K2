import { useLayoutEffect, useRef, type JSX, type RefObject } from 'react'

// Thread view without a top bar (prototype): the header's controls become
// small objects floating over the conversation, as on the phone. The header
// stays where it is in the DOM (`data-session-header` still finds the pane);
// CSS lifts it out of the flow. The look is in globals.css ("Thread view's
// floating objects"). Terminal: chamfered and solid. Glass: Frost, from
// `data-material="frost"` (styles/glass/overrides.css).
//
// Values go on the pane, for what's inside it:
// - the floats' height, which the Thread's list pads by so the first message
//   starts below them and the rest scroll under;
// - how far the revealed terminal reaches from the left or top, which keeps
//   the floats over the Thread and off the terminal.

/** Marks the element the two values live on (the header's parent). */
export const THREAD_FLOATS_HOST_ATTR = 'data-thread-floats-host'
export const THREAD_FLOAT_INSET_VAR = '--thread-float-inset'
export const THREAD_FLOAT_LEFT_VAR = '--thread-float-left'
export const THREAD_FLOAT_TOP_VAR = '--thread-float-top'

/** The sides can't shrink below their buttons, so the name's pill can't run
 *  under them: it truncates instead. With room to spare the sides are equal
 *  and the name is centred; only when it is out of room does it sit off-centre. */
export const THREAD_FLOATS_GRID =
  'grid grid-cols-[minmax(max-content,1fr)_minmax(0,auto)_minmax(max-content,1fr)] gap-x-2 items-start px-3'

/** `capsule`: several controls in one object, the page tabs' pattern
 *  (`ThreadDrawerSelector`). */
export type ThreadFloatShape = 'round' | 'pill' | 'name' | 'note' | 'capsule'

/** Where a pane's floating objects are drawn: over the Thread on its own
 *  (Thread view) or over the Chatter on its own (Chatter view). Split view
 *  has no floats: one bar across both columns (splitChrome.tsx). */
export type ThreadFloatsPlace = 'thread' | 'chatter'

/** Each floating object, by name. */
export type ThreadFloatId =
  | 'view'
  /** One capsule: the terminal, Chatter and Heartbeats drawers. */
  | 'drawers'
  | 'bell'
  | 'search'
  | 'identity'
  | 'token'
  | 'history'
  | 'refresh'

/** The only objects Chatter view shows (2026-11-10, "look nice like the
 *  rest of our threads"): Thread view's header without what belongs to the
 *  Thread itself. The view menu (the way out), the agent's picture and name,
 *  search over the chatter, and on the right restart, the pinned chat's
 *  session and Token. No terminal toggle or drawers (Thread view's reveal),
 *  and no bell (it rings for the agent's turns with you, not for chatter). */
export const CHATTER_VIEW_FLOATS: ReadonlySet<ThreadFloatId> = new Set<ThreadFloatId>([
  'view',
  'identity',
  'search',
  'token',
  'history',
  'refresh',
])

/** Whether `id` is drawn in `place`: Thread view shows them all. */
export function threadFloatShown(id: ThreadFloatId, place: ThreadFloatsPlace): boolean {
  if (place === 'thread') return true
  return CHATTER_VIEW_FLOATS.has(id)
}

/** A floating object: chamfered in Terminal, Frost in Glass. Off, it changes
 *  nothing. No other style reads the attributes. */
export function floatProps(floating: boolean, shape: ThreadFloatShape): Record<string, string> {
  if (!floating) return {}
  return {
    'data-k2-float': shape,
    'data-k2-chamfer': 'control',
    'data-material': 'frost',
    'data-frost-shape': shape === 'round' ? 'round' : 'pill',
  }
}

/** The element a floating header's values live on: its parent. */
export function threadFloatsHostOf(header: Element): HTMLElement | null {
  return header.parentElement
}

/** Ref for the header. While floating, publishes its height on its host. */
export function useThreadFloats(floating: boolean): RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const header = ref.current
    const host = header ? threadFloatsHostOf(header) : null
    if (!floating || !header || !host) return
    host.setAttribute(THREAD_FLOATS_HOST_ATTR, '')
    const publish = (): void => {
      host.style.setProperty(THREAD_FLOAT_INSET_VAR, `${Math.ceil(header.getBoundingClientRect().height)}px`)
    }
    publish()
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(publish)
    ro?.observe(header)
    return () => {
      ro?.disconnect()
      host.removeAttribute(THREAD_FLOATS_HOST_ATTR)
      host.style.removeProperty(THREAD_FLOAT_INSET_VAR)
    }
  }, [floating])
  return ref
}

/** From the revealed terminal's column: how far it reaches into the pane.
 *  The floats belong to the Thread, so they sit beside or below the terminal,
 *  never over it. */
export function useThreadFloatsClearOfTerminal(
  container: RefObject<HTMLElement | null>,
  active: boolean,
  { left, top }: { left: number; top: number },
): void {
  useLayoutEffect(() => {
    const host = active ? container.current?.closest<HTMLElement>(`[${THREAD_FLOATS_HOST_ATTR}]`) : null
    if (!host) return
    host.style.setProperty(THREAD_FLOAT_LEFT_VAR, `${left}px`)
    host.style.setProperty(THREAD_FLOAT_TOP_VAR, `${top}px`)
    return () => {
      host.style.removeProperty(THREAD_FLOAT_LEFT_VAR)
      host.style.removeProperty(THREAD_FLOAT_TOP_VAR)
    }
  }, [container, active, left, top])
}

/** The pane's floating header, from inside the pane. */
export function threadFloatsIn(container: HTMLElement): HTMLElement | null {
  return (
    container
      .closest(`[${THREAD_FLOATS_HOST_ATTR}]`)
      ?.querySelector<HTMLElement>(':scope > [data-thread-floats]') ?? null
  )
}

/** Behind the floats, outside Glass: the conversation fades as it passes
 *  under, with no blur. Glass has the real band instead (ProgressiveHeaderBlur,
 *  in the Thread's own box), so this draws nothing there. Never takes clicks. */
export function ThreadFloatScrim(): JSX.Element {
  return <div className="thread-float-scrim" data-testid="thread-float-scrim" aria-hidden="true" />
}
