// `k2.open(url)` (0.46.4): K2's own card the first time a widget opens a
// link to a site from this Garden. K2's DOM laid over the widget's card,
// outside the sealed frame (the G8 pattern, like ZenGuestRoomsSheet): the
// widget can't draw it, click it or read it, and every word is K2's; the
// widget gave only the link. Zen tokens only (T4.3).
//
// Don't open (also Esc) answers the widget `{ok: false}`; Open once opens
// it this time; Always open keeps the site for this Garden on this
// computer (the daemon's list; `k2 zen links forget` asks again).

import { useEffect, useRef } from 'react'
import type { ZenLinkAnswer, ZenLinkAsk } from '@/lib/zen/zen-links'
import { zenButtonStyle } from './ZenOverlay'

const muted: React.CSSProperties = { color: 'var(--zen-text-muted)' }

export interface ZenLinkSheetProps {
  ask: ZenLinkAsk
  onDone: (answer: ZenLinkAnswer) => void
}

/** The site as people read it: the host, with the port when not the default. */
export function zenLinkSiteLabel(site: string): string {
  try {
    return new URL(site).host
  } catch {
    return site
  }
}

export function ZenLinkSheet({ ask, onDone }: ZenLinkSheetProps): React.JSX.Element {
  const first = useRef<HTMLButtonElement | null>(null)
  useEffect(() => {
    first.current?.focus()
  }, [])
  const host = zenLinkSiteLabel(ask.site)
  const insecure = ask.site.startsWith('http:')
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={`Open a link to ${host}?`}
      data-testid="zen-link-sheet"
      className="absolute inset-0 z-20 flex items-center justify-center"
      style={{ padding: 12, background: 'rgba(0, 0, 0, 0.4)' }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onDone('deny')
        }
      }}
    >
      <div
        className="w-full"
        style={{
          maxWidth: 360,
          padding: 12,
          fontSize: 12,
          color: 'var(--zen-text)',
          background: 'var(--zen-surface-raised)',
          border: '1px solid var(--zen-border)',
          display: 'flex',
          flexDirection: 'column',
          gap: 8,
        }}
      >
        <p style={{ fontWeight: 500 }}>
          The widget <span data-testid="zen-link-widget">{ask.widget}</span> wants to open a link to{' '}
          <span data-testid="zen-link-host">{host}</span> in your browser.
        </p>
        <p data-testid="zen-link-url" style={{ ...muted, fontSize: 11, wordBreak: 'break-all' }}>
          {ask.url}
        </p>
        {insecure && (
          <p data-testid="zen-link-insecure" style={{ ...muted, fontSize: 11 }}>
            This link isn’t encrypted (http).
          </p>
        )}
        <div className="flex flex-wrap" style={{ gap: 6 }}>
          <button
            ref={first}
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(false)}
            data-testid="zen-link-deny"
            onClick={() => onDone('deny')}
          >
            Don’t open
          </button>
          <button
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(true)}
            data-testid="zen-link-once"
            onClick={() => onDone('once')}
          >
            Open once
          </button>
          <button
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(false)}
            data-testid="zen-link-always"
            onClick={() => onDone('always')}
          >
            Always open {host} from {ask.garden}
          </button>
        </div>
      </div>
    </div>
  )
}
