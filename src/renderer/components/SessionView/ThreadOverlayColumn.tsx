import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { SelectableRegion } from '@/components/common/SelectableText'
import { focusThreadComposeSlotTextarea } from '@/lib/workspace-switch-focus'
import { useSettingsStore } from '@/stores/settings'
import { ThreadOverlayPane, threadZenScopeProps } from './ThreadOverlayPane'
import type { ThreadTurn } from './overlayThread'
import { ThreadComposeDrawerContext, type ThreadComposeDrawer } from './threadComposeDrawer'
import { FOOTER_FADE_TARGETS, ThreadFooterFade, threadFadeTop } from '@/components/Thread/ThreadZenKit'

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
 */
export function ThreadOverlayColumn({
  addr,
  conversationId,
  active,
  composeBar,
  split,
  agentName,
  onStop,
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
}): React.JSX.Element {
  const composeRef = useRef<HTMLDivElement>(null)
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

  const scope = threadZenScopeProps(editorFontSize)
  return (
    <div
      className={
        split
          ? 'relative flex-1 min-w-0 min-h-0 overflow-hidden border-l border-[var(--color-border)]'
          : 'relative flex-1 min-w-0 min-h-0 overflow-hidden'
      }
      data-testid="agent-session-thread"
      data-compose-drop-surface="thread"
      {...scope}
    >
      <div
        className="absolute inset-0 overflow-hidden flex flex-col"
        data-testid="agent-session-thread-list-slot"
      >
        <SelectableRegion className="h-full min-h-0 flex flex-col overflow-hidden">
          <ThreadOverlayPane
            addr={addr}
            conversationId={conversationId}
            active={active}
            agentName={agentName}
            onStop={onStop}
            onTurn={composeShown ? setTurn : undefined}
            bottomInset={composeShown ? bottom : 0}
          />
        </SelectableRegion>
      </div>
      {hasCompose ? (
        <div
          ref={composeRef}
          className="absolute left-0 right-0 bottom-0"
          data-testid="agent-session-thread-compose-slot"
          onMouseDown={(e) => {
            focusThreadComposeSlotTextarea(e.currentTarget, e.target)
          }}
        >
          {composeShown ? <ThreadFooterFade top={fadeTop} /> : null}
          <div className="relative" style={{ zIndex: 1 }}>
            <ThreadComposeDrawerContext.Provider value={drawer}>{composeBar}</ThreadComposeDrawerContext.Provider>
          </div>
        </div>
      ) : null}
    </div>
  )
}
