// `k2.download` (prd-garden-board-needs-v1; security review #1): K2's own
// card the first time a widget saves a file to Downloads from a Garden. K2's
// DOM laid over the widget's card, outside the sealed frame (the G8 pattern,
// like ZenLinkSheet): the widget can't draw it, click it or read it, and
// every word is K2's; the widget gave only the file name (shown as text).
//
// Don't save (also Esc) answers the widget `{saved: false}`; Save once
// saves it this time; Always allow keeps the answer for this widget in this
// Garden on this computer.

import { useEffect, useRef } from 'react'
import type { ZenDownloadAnswer, ZenDownloadAsk } from '@/lib/zen/zen-download-consent'
import { zenButtonStyle } from './ZenOverlay'

const muted: React.CSSProperties = { color: 'var(--zen-text-muted)' }

export interface ZenDownloadSheetProps {
  ask: ZenDownloadAsk
  onDone: (answer: ZenDownloadAnswer) => void
}

/** A size as people read it. */
export function zenDownloadSizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function ZenDownloadSheet({ ask, onDone }: ZenDownloadSheetProps): React.JSX.Element {
  const first = useRef<HTMLButtonElement | null>(null)
  useEffect(() => {
    first.current?.focus()
  }, [])
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={`Save a file from ${ask.widget}?`}
      data-testid="zen-download-sheet"
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
          The widget <span data-testid="zen-download-widget">{ask.widget}</span> wants to save a file to your Downloads folder.
        </p>
        <p data-testid="zen-download-name" style={{ ...muted, fontSize: 11, wordBreak: 'break-all' }}>
          {ask.name} ({zenDownloadSizeLabel(ask.size)})
        </p>
        <div className="flex flex-wrap" style={{ gap: 6 }}>
          <button
            ref={first}
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(false)}
            data-testid="zen-download-deny"
            onClick={() => onDone('deny')}
          >
            Don’t save
          </button>
          <button
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(true)}
            data-testid="zen-download-once"
            onClick={() => onDone('once')}
          >
            Save once
          </button>
          <button
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(false)}
            data-testid="zen-download-always"
            onClick={() => onDone('always')}
          >
            Always let {ask.widget} save files from {ask.garden}
          </button>
        </div>
      </div>
    </div>
  )
}
