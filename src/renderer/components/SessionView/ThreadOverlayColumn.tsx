import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { SelectableRegion } from '@/components/common/SelectableText'
import { focusThreadComposeSlotTextarea } from '@/lib/workspace-switch-focus'
import { useSettingsStore } from '@/stores/settings'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { ThreadOverlayPane, threadZenScopeProps, type ThreadFill } from './ThreadOverlayPane'
import type { ThreadTurn } from './overlayThread'
import { ThreadComposeDrawerContext, type ThreadComposeDrawer } from './threadComposeDrawer'
import { ThreadEmptyStart, emptyStartPlace, sameEmptyStartPlace, type EmptyStartPlace } from './ThreadEmptyStart'
import { THREAD_FLOAT_INSET_VAR } from './threadFloats'
import { FOOTER_FADE_TARGETS, ThreadFooterFade, threadFadeTop } from '@/components/Thread/ThreadZenKit'
import { ProgressiveHeaderBlur } from '@/components/ui'

/** The drift's length when the Style's `--motion-duration-slow` can't be read. */
export const EMPTY_START_DRIFT_FALLBACK_MS = 320
/** Past the drift's end before the column lets go of it (a frame or two). */
const EMPTY_START_DRIFT_SLACK_MS = 80

/** A CSS time (`320ms`, `0.3s`) in ms; null when it isn't one. */
export function cssTimeMs(raw: string): number | null {
  const m = /^\s*([\d.]+)\s*(ms|s)\s*$/.exec(raw)
  if (!m) return null
  const n = Number(m[1])
  if (!Number.isFinite(n)) return null
  return m[2] === 's' ? n * 1000 : n
}

/**
 * Thread list + Message-the-agent bar. The bar floats over the list's
 * bottom on a see-through fade (K2 Companion's footer fade): the list
 * fills the column and pads its bottom by the bar's measured height, so
 * messages scroll under the bar and the newest rests right above it.
 *
 * `data-compose-drop-surface` is the image-drop hit region for the bar
 * inside this column (external-drop-router / file-drag). Non-image drops
 * are not claimed.
 *
 * The column is a Zen Thread box (`data-k2-thread-zen` + the `--zen-*`
 * tokens from the app's Style), so the list and the bar share the Zen
 * Thread widget's look. While the bar shows, the activity drawer rides on
 * its top edge (the list hands the live turn up, the bar reads it from
 * `ThreadComposeDrawerContext`); without a bar the list draws it itself.
 *
 * `centreEmpty` (Thread view and split view's Thread column): a Thread that
 * has loaded and has nothing in it starts centred (ThreadEmptyStart), the
 * bar lifted by a transform under the agent's picture and name. On the
 * first message the transform goes back to none with a transition (the
 * Style's slow motion), so the bar slides down as one layer: nothing
 * re-mounts or re-lays out, and the field keeps its focus and caret.
 * Reduced motion: an instant switch.
 */
export function ThreadOverlayColumn({
  addr,
  conversationId,
  active,
  composeBar,
  split,
  agentName,
  onStop,
  textCapPx = null,
  centreEmpty = false,
}: {
  addr: string
  conversationId: string | null
  active: boolean
  composeBar: ReactNode | null
  split?: boolean
  /** Shown in the empty state ("Message <name>. It reads this like a text."). */
  agentName?: string
  /** The working strip's Stop: Esc to this session's PTY. */
  onStop?: () => void
  /** Thread view's text limit (px); Split and Design leave it off. Caps the
   *  messages and the bar, centred. The fade stays full width. */
  textCapPx?: number | null
  /** An empty Thread starts centred, picture and name over the bar. */
  centreEmpty?: boolean
}): React.JSX.Element {
  const rootRef = useRef<HTMLDivElement>(null)
  const composeRef = useRef<HTMLDivElement>(null)
  const boxRef = useRef<HTMLDivElement>(null)
  const startRef = useRef<HTMLDivElement>(null)
  const [bottom, setBottom] = useState(0)
  /** The fade's top: just above the drawer's tab when it is out, else the
   *  field (measured with the bar, so it moves when they do). */
  const [fadeTop, setFadeTop] = useState<number | null>(null)
  const [turn, setTurn] = useState<ThreadTurn | null>(null)
  const hasCompose = composeBar != null
  // A bar that renders nothing (a view-only room, remote instruct off)
  // measures 0: the list keeps the drawer then.
  const composeShown = hasCompose && bottom > 0
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13
  const drawer = useMemo<ThreadComposeDrawer>(() => ({ turn, onStop }), [turn, onStop])
  const reducedMotion = useReducedMotion()

  // The empty start. `fill` from the list (before paint); `place` measured
  // here; `centred` only once it fits, so a loading or tiny column never
  // shows it.
  const [fill, setFill] = useState<ThreadFill>('loading')
  const onFill = useCallback((next: ThreadFill) => setFill(next), [])
  const startWanted = centreEmpty && composeShown && fill === 'empty'
  const [place, setPlace] = useState<EmptyStartPlace | null>(null)
  const centred = startWanted && place != null && place.fits
  // The drift is decided in render, in the same commit that drops the lift:
  // the transition and the new transform land in one style change.
  const [drawnCentred, setDrawnCentred] = useState(false)
  const [drifting, setDrifting] = useState(false)
  if (drawnCentred !== centred) {
    setDrawnCentred(centred)
    setDrifting(!centred && fill === 'some' && !reducedMotion)
  }
  const startDrawn = startWanted || drifting

  useLayoutEffect(() => {
    const el = composeRef.current
    if (!el || !hasCompose) {
      setBottom(0)
      return
    }
    const measure = (): void => {
      const slot = el.getBoundingClientRect()
      setBottom(Math.ceil(slot.height))
      // Everything that can sit on the box is in the bar's flow, so its
      // coming and going (and the drawer growing) resizes the slot: this
      // one observer re-measures it. Reads only, then one state write.
      const tops = FOOTER_FADE_TARGETS.flatMap((sel) => {
        const node = el.querySelector(sel)
        return node ? [node.getBoundingClientRect().top] : []
      })
      setFadeTop(threadFadeTop(slot.top, tops))
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [hasCompose])

  // Place the group: the column, the bar and the picture-and-name, each
  // by its layout size (a transform doesn't change it), so lifting the bar
  // can't feed back into the measure.
  useLayoutEffect(() => {
    if (!startWanted) return
    const root = rootRef.current
    const box = boxRef.current
    const start = startRef.current
    if (!root || !box || !start) return
    const measure = (): void => {
      const inset = Number.parseFloat(getComputedStyle(root).getPropertyValue(THREAD_FLOAT_INSET_VAR)) || 0
      const next = emptyStartPlace({
        width: root.clientWidth,
        height: root.clientHeight,
        inset,
        box: box.offsetHeight,
        hero: start.offsetHeight,
      })
      setPlace((prev) => (sameEmptyStartPlace(prev, next) ? prev : next))
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(root)
    ro.observe(box)
    ro.observe(start)
    return () => ro.disconnect()
  }, [startWanted, bottom])

  // The drift ends with the Style's slow motion (the bar's transition).
  useEffect(() => {
    if (!drifting) return
    const root = rootRef.current
    const ms = (root && cssTimeMs(getComputedStyle(root).getPropertyValue('--motion-duration-slow'))) ?? EMPTY_START_DRIFT_FALLBACK_MS
    const id = setTimeout(() => setDrifting(false), ms + EMPTY_START_DRIFT_SLACK_MS)
    return () => clearTimeout(id)
  }, [drifting])

  const lift = centred && place ? place.lift : 0
  const capWidth = textCapPx != null ? `calc(${textCapPx}px + 2 * var(--zen-gap, 12px))` : undefined
  const scope = threadZenScopeProps(editorFontSize)
  return (
    <div
      ref={rootRef}
      className={
        split
          ? 'relative flex-1 min-w-0 min-h-0 overflow-hidden border-l border-[var(--color-border)]'
          : 'relative flex-1 min-w-0 min-h-0 overflow-hidden'
      }
      data-testid="agent-session-thread"
      data-compose-drop-surface="thread"
      data-thread-centred={centred ? '' : undefined}
      data-thread-drift={drifting ? '' : undefined}
      {...scope}
    >
      <div
        className="absolute inset-0 overflow-hidden flex flex-col"
        data-testid="agent-session-thread-list-slot"
      >
        <ProgressiveHeaderBlur />
        <SelectableRegion className="h-full min-h-0 flex flex-col overflow-hidden">
          <ThreadOverlayPane
            addr={addr}
            conversationId={conversationId}
            active={active}
            agentName={agentName}
            onStop={onStop}
            onTurn={composeShown ? setTurn : undefined}
            bottomInset={composeShown ? bottom : 0}
            textCapPx={textCapPx}
            onFill={centreEmpty ? onFill : undefined}
            emptyLine={!centred}
          />
        </SelectableRegion>
      </div>
      {startDrawn ? (
        <ThreadEmptyStart
          ref={startRef}
          name={agentName?.trim() || addr}
          addr={addr}
          top={place?.fits ? place.top : 0}
          small={place?.small ?? false}
          shown={centred}
          maxWidth={capWidth}
        />
      ) : null}
      {hasCompose ? (
        <div
          ref={composeRef}
          className="absolute left-0 right-0 bottom-0"
          data-testid="agent-session-thread-compose-slot"
          data-thread-compose-slot=""
          style={lift > 0 ? { transform: `translateY(${-lift}px)` } : undefined}
          onMouseDown={(e) => {
            focusThreadComposeSlotTextarea(e.currentTarget, e.target)
          }}
        >
          {composeShown ? <ThreadFooterFade top={fadeTop} /> : null}
          <div
            ref={boxRef}
            className="relative"
            data-testid="agent-session-thread-compose-box"
            style={
              capWidth != null
                ? {
                    zIndex: 1,
                    width: '100%',
                    // The bar pads by --zen-gap, as the list does: its field
                    // lines up with the messages at the same limit.
                    maxWidth: capWidth,
                    marginInline: 'auto',
                  }
                : { zIndex: 1 }
            }
          >
            <ThreadComposeDrawerContext.Provider value={drawer}>{composeBar}</ThreadComposeDrawerContext.Provider>
          </div>
        </div>
      ) : null}
    </div>
  )
}
