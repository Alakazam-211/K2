import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { ChatOverlayPane } from './ChatOverlayPane'
import type { SessionViewTab } from './sessionViewTab'

/**
 * Message list + the existing Message-the-agent bar. Absolute inset so the
 * bar's height shortens the list (same dock as Thread). The parent passes
 * `TerminalComposeBar` with `sendDestination="pty"` and the daemon PTY id.
 */
export function ChatOverlayColumn({
  view,
  visible,
  provider,
  conversationId,
  agentName,
  composeBar,
}: {
  view: SessionViewTab
  visible: boolean
  provider: string | null
  conversationId: string | null
  agentName: string | null
  composeBar: ReactNode | null
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
      className="relative flex-1 min-w-0 min-h-0 overflow-hidden"
      data-testid="chat-overlay-column"
    >
      <div
        className="absolute left-0 right-0 top-0 overflow-hidden flex flex-col"
        style={{ bottom }}
        data-testid="chat-overlay-list-slot"
      >
        <ChatOverlayPane
          view={view}
          visible={visible}
          provider={provider}
          conversationId={conversationId}
          agentName={agentName}
        />
      </div>
      {hasCompose ? (
        <div
          ref={composeRef}
          className="absolute left-0 right-0 bottom-0"
          data-testid="chat-overlay-compose-slot"
        >
          {composeBar}
        </div>
      ) : null}
    </div>
  )
}
