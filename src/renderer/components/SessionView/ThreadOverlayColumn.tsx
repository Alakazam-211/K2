import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { SelectableRegion } from '@/components/common/SelectableText'
import { focusThreadComposeSlotTextarea } from '@/lib/workspace-switch-focus'
import { useSettingsStore } from '@/stores/settings'
import { ThreadOverlayPane, threadZenScopeProps } from './ThreadOverlayPane'
import type { ThreadTurn } from './overlayThread'
import { ThreadComposeDrawerContext, type ThreadComposeDrawer } from './threadComposeDrawer'

/**
 * Thread list + Message-the-agent bar. Absolute inset so the bar's
 * used height always shortens the list — WKWebView does not shrink a
 * flex-1 / height:100% scrollport when a sibling textarea grows.
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
      setBottom(Math.ceil(el.getBoundingClientRect().height))
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
        className="absolute left-0 right-0 top-0 overflow-hidden flex flex-col"
        style={{ bottom }}
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
          <ThreadComposeDrawerContext.Provider value={drawer}>{composeBar}</ThreadComposeDrawerContext.Provider>
        </div>
      ) : null}
    </div>
  )
}
