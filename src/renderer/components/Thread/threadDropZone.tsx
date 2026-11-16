// The whole Thread pane takes a dropped file (Rosson 2026-11-16): a file
// dropped on the messages, the header, the empty state or the message box
// goes to the box's attachments, through the box's own drop handler (same
// size limits, refusals and image previews as a drop on the box).
//
// - HTML5 drags (the web client, and any drag the page sees): only drags
//   that carry Files. Text, link and in-window drags (tabs, cards,
//   reordering) pass by untouched. A depth counter plus `relatedTarget`
//   keeps the drop state steady while the pointer crosses child elements.
// - Tauri OS drags: the page sees no HTML5 drag; external-drop-router marks
//   the pane under the pointer (`COMPOSE_SURFACE_DROP_OVER_EVENT`) and
//   routes the drop to the pane's box itself.
//
// A pane whose box takes no files (no box, view-only, disabled) offers no
// drop: no handlers, no drop state.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type JSX,
  type RefObject,
} from 'react'
import {
  COMPOSE_SURFACE_DROP_OVER_EVENT,
  type ComposeSurfaceDropOverDetail,
} from '@/lib/compose-surface-drop'

/** A message box's own drop handler (it reads `e.dataTransfer`). */
export type ThreadDropHandler = (e: DragEvent<HTMLDivElement>) => void

/** The drag carries files from outside the page (not text, a link or an
 *  in-window drag). */
export function dragCarriesFiles(dt: DataTransfer | null | undefined): boolean {
  const types = dt?.types as unknown as
    | (ArrayLike<string> & { includes?: (t: string) => boolean; contains?: (t: string) => boolean })
    | undefined
  if (!types) return false
  if (typeof types.includes === 'function') return types.includes('Files')
  if (typeof types.contains === 'function') return types.contains('Files')
  return false
}

export interface ThreadDropZone {
  /** A file drag is over the pane (and the pane takes it). */
  over: boolean
  /** Spread on the pane's root. Empty when the pane takes no drop. */
  handlers: {
    onDragEnter?: (e: DragEvent<HTMLDivElement>) => void
    onDragOver?: (e: DragEvent<HTMLDivElement>) => void
    onDragLeave?: (e: DragEvent<HTMLDivElement>) => void
    onDrop?: (e: DragEvent<HTMLDivElement>) => void
    onDropCapture?: (e: DragEvent<HTMLDivElement>) => void
  }
}

/** The pane at `rootRef` takes file drops into `onDrop` (null: it takes
 *  none). */
export function useThreadDropZone(
  rootRef: RefObject<HTMLElement | null>,
  onDrop: ThreadDropHandler | null | undefined,
): ThreadDropZone {
  const enabled = onDrop != null
  const depth = useRef(0)
  const [htmlOver, setHtmlOver] = useState(false)
  const [osOver, setOsOver] = useState(false)
  const dropRef = useRef(onDrop)
  dropRef.current = onDrop

  const reset = useCallback(() => {
    depth.current = 0
    setHtmlOver(false)
  }, [])

  // Off: forget any drag in progress.
  useEffect(() => {
    if (!enabled) {
      depth.current = 0
      setHtmlOver(false)
      setOsOver(false)
    }
  }, [enabled])

  // Tauri OS drags: the router marks this pane.
  useEffect(() => {
    const el = rootRef.current
    if (!el || !enabled) return
    const onMark = (e: Event): void => {
      setOsOver((e as CustomEvent<ComposeSurfaceDropOverDetail>).detail?.over === true)
    }
    el.addEventListener(COMPOSE_SURFACE_DROP_OVER_EVENT, onMark)
    return () => el.removeEventListener(COMPOSE_SURFACE_DROP_OVER_EVENT, onMark)
  }, [rootRef, enabled])

  const handlers = useMemo<ThreadDropZone['handlers']>(() => {
    if (!enabled) return {}
    return {
      onDragEnter: (e) => {
        if (!dragCarriesFiles(e.dataTransfer)) return
        e.preventDefault()
        depth.current += 1
        setHtmlOver(true)
      },
      onDragOver: (e) => {
        if (!dragCarriesFiles(e.dataTransfer)) return
        // Without preventDefault the browser never fires drop (and opens
        // the file instead).
        e.preventDefault()
        e.stopPropagation()
        e.dataTransfer.dropEffect = 'copy'
      },
      onDragLeave: (e) => {
        if (!dragCarriesFiles(e.dataTransfer)) return
        depth.current = Math.max(0, depth.current - 1)
        const root = rootRef.current
        const next = e.relatedTarget as Node | null
        // Still inside the pane (onto a child): stay. Out of it: off.
        if (next && root && typeof (next as Node).nodeType === 'number' && root.contains(next)) return
        if (next || depth.current === 0) reset()
      },
      // A drop on the box itself is the box's (it stops it there): the
      // pane only clears its drop state, on the way down.
      onDropCapture: (e) => {
        if (dragCarriesFiles(e.dataTransfer)) reset()
      },
      onDrop: (e) => {
        if (!dragCarriesFiles(e.dataTransfer)) return
        // Never navigate to (open) the file, never reach another pane.
        e.preventDefault()
        e.stopPropagation()
        reset()
        dropRef.current?.(e)
      },
    }
  }, [enabled, reset, rootRef])

  return { over: enabled && (htmlOver || osOver), handlers }
}

/** The pane's drop state: the Garden drop ring's dashed accent outline
 *  over the whole pane, and "Drop to attach". Zen tokens, so it follows
 *  the Style (Terminal and Glass). Never takes the pointer. */
export function ThreadDropOverlay({ shown }: { shown: boolean }): JSX.Element | null {
  if (!shown) return null
  return (
    <div
      aria-hidden
      data-testid="thread-drop-overlay"
      data-thread-drop-overlay=""
      className="pointer-events-none absolute inset-0 flex items-center justify-center"
      style={{
        zIndex: 40,
        outline: '2px dashed var(--zen-accent, var(--color-accent))',
        outlineOffset: -2,
        borderRadius: 'inherit',
      }}
    >
      <span
        style={{
          padding: '6px 12px',
          borderRadius: 'var(--zen-pill-radius, 999px)',
          border: '1px solid var(--zen-border, var(--color-border))',
          background: 'var(--zen-surface-raised, var(--color-bg-surface))',
          color: 'var(--zen-text, var(--color-text-primary))',
          fontSize: 'var(--zen-type-sm, 12px)',
        }}
      >
        Drop to attach
      </span>
    </div>
  )
}

/** A Thread pane hands its message box a slot: the box puts its drop
 *  handler there, and the pane takes drops into it (ThreadOverlayColumn). */
export interface ThreadComposeDropSlot {
  set: (handler: ThreadDropHandler) => void
  clear: (handler: ThreadDropHandler) => void
}

export const ThreadComposeDropContext = createContext<ThreadComposeDropSlot | null>(null)

/** The pane's side: the handler its box put in the slot, and the slot. */
export function useThreadComposeDropSlot(): { handler: ThreadDropHandler | null; slot: ThreadComposeDropSlot } {
  const [handler, setHandler] = useState<ThreadDropHandler | null>(null)
  const slot = useMemo<ThreadComposeDropSlot>(
    () => ({
      set: (h) => setHandler(() => h),
      clear: (h) => setHandler((cur: ThreadDropHandler | null) => (cur === h ? null : cur)),
    }),
    [],
  )
  return { handler, slot }
}

/** The box's side: while `enabled`, the pane around it (if any) takes
 *  drops into `handler`, the box's own. */
export function useThreadComposeDropSource(enabled: boolean, handler: ThreadDropHandler): void {
  const slot = useContext(ThreadComposeDropContext)
  const ref = useRef(handler)
  ref.current = handler
  useEffect(() => {
    if (!slot || !enabled) return
    const h: ThreadDropHandler = (e) => ref.current(e)
    slot.set(h)
    return () => slot.clear(h)
  }, [slot, enabled])
}
