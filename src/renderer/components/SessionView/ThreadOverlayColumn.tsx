import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { SelectableRegion } from '@/components/common/SelectableText'
import { ThreadOverlayPane } from './ThreadOverlayPane'

/**
 * Thread list + Message-the-agent bar. Absolute inset so the bar's
 * used height always shortens the list — WKWebView does not shrink a
 * flex-1 / height:100% scrollport when a sibling textarea grows.
 */
export function ThreadOverlayColumn({
  addr,
  conversationId,
  active,
  composeBar,
  split,
}: {
  addr: string
  conversationId: string | null
  active: boolean
  composeBar: ReactNode | null
  split?: boolean
}): React.JSX.Element {
  const composeRef = useRef<HTMLDivElement>(null)
  const [bottom, setBottom] = useState(0)
  const hasCompose = composeBar != null

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

  return (
    <div
      className={
        split
          ? 'relative flex-1 min-w-0 min-h-0 overflow-hidden border-l border-[var(--color-border)]'
          : 'relative flex-1 min-w-0 min-h-0 overflow-hidden'
      }
      data-testid="agent-session-thread"
    >
      <div
        className="absolute left-0 right-0 top-0 overflow-hidden flex flex-col"
        style={{ bottom }}
        data-testid="agent-session-thread-list-slot"
      >
        <SelectableRegion className="h-full min-h-0 flex flex-col overflow-hidden">
          <ThreadOverlayPane addr={addr} conversationId={conversationId} active={active} />
        </SelectableRegion>
      </div>
      {hasCompose ? (
        <div
          ref={composeRef}
          className="absolute left-0 right-0 bottom-0"
          data-testid="agent-session-thread-compose-slot"
        >
          {composeBar}
        </div>
      ) : null}
    </div>
  )
}
