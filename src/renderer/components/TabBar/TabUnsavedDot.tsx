// A file tab with unsaved changes: a dot after its name (notes §2 Saving,
// §8, x04), in place of the old italics. Hovering it says "Unsaved changes.
// Close saves them first." in K2's tooltip, under the tab (in a portal: the
// tab strip clips). The × stays its own button beside it, and closing saves
// first (FileViewerPane's leave write; a write that can't land keeps the
// words as this computer's draft).

import { useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { floatMaterial } from '@/components/ui/float-material'
import { clientRectToCss } from '@/lib/app-zoom'
import { useBrowserOverlay } from '@/lib/browser-overlay'

export const UNSAVED_TAB_HINT = 'Unsaved changes. Close saves them first.'

const TIP: React.CSSProperties = {
  position: 'fixed',
  transform: 'translateX(-50%)',
  zIndex: 60,
  padding: '4px 8px',
  whiteSpace: 'nowrap',
  fontSize: 'var(--type-sm)',
  color: 'var(--color-text-primary)',
  background: 'var(--color-bg-elevated)',
  border: '1px solid var(--color-border-strong)',
  borderRadius: 'var(--radius-field)',
  boxShadow: 'var(--shadow-2)',
  pointerEvents: 'none',
}

export function TabUnsavedDot({ dirty }: { dirty: boolean }): React.JSX.Element | null {
  const [at, setAt] = useState<{ x: number; y: number } | null>(null)
  const tip = useRef<HTMLSpanElement>(null)
  // The tooltip can sit over a Browser tab's native page: claim its box.
  useBrowserOverlay({ active: dirty && at !== null, ref: tip, kind: 'popover', watch: at })
  if (!dirty) return null
  return (
    <span
      data-tab-unsaved=""
      role="img"
      aria-label={UNSAVED_TAB_HINT}
      onMouseEnter={(e) => {
        const r = clientRectToCss(e.currentTarget.getBoundingClientRect())
        setAt({ x: r.left + r.width / 2, y: r.bottom + 6 })
      }}
      onMouseLeave={() => setAt(null)}
      className="ml-1 flex h-full flex-shrink-0 items-center self-stretch px-0.5"
    >
      <svg aria-hidden="true" width="7" height="7" viewBox="0 0 8 8" className="block text-[var(--color-text-primary)]">
        <circle cx="4" cy="4" r="4" fill="currentColor" />
      </svg>
      {at
        ? createPortal(
            <span ref={tip} role="tooltip" data-tab-unsaved-tip="" style={{ ...TIP, left: at.x, top: at.y }} {...floatMaterial('popover')}>
              {UNSAVED_TAB_HINT}
            </span>,
            document.body,
          )
        : null}
    </span>
  )
}
