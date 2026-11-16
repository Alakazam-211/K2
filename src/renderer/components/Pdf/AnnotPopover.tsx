// An annotation's popover, on the page beside it: who (from /T), when (from
// /M or /CreationDate), what kind, and its text. Escape, the close button or
// a click elsewhere closes it; the keyboard goes back where it came from.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { floatMaterial } from '@/components/ui'
import { CloseGlyph } from '@/components/LiveView/shell/glyphs'
import type { PdfAnnot } from '@/lib/pdf/annotations'
import { formatAnnotDate } from './annot-format'

/** Its width, narrower when the desk is (an 800 px window with the panel open). */
const W = 280

export function AnnotPopover({ annot, box, pageW, pageH, onClose }: { annot: PdfAnnot; box: number[]; pageW: number; pageH: number; onClose: () => void }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const [w, setW] = useState(W)
  // As wide as the desk shows, at most W; then scrolled into view whole.
  useLayoutEffect(() => {
    const desk = ref.current?.closest('.pdf-desk')
    if (desk && desk.clientWidth > 0) setW(Math.max(160, Math.min(W, desk.clientWidth - 16)))
  }, [annot.key])
  useEffect(() => {
    ref.current?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  }, [annot.key, w])
  useEffect(() => {
    const back = document.activeElement as HTMLElement | null
    ref.current?.focus({ preventScroll: true })
    const away = (e: MouseEvent): void => {
      const t = e.target as Element
      if (ref.current?.contains(t) || t.closest?.('section[data-annotation-id], [data-testid="pdf-annot-row"]')) return
      onClose()
    }
    document.addEventListener('mousedown', away)
    return () => {
      document.removeEventListener('mousedown', away)
      if (back?.isConnected && ref.current?.contains(document.activeElement)) back.focus({ preventScroll: true })
    }
  }, [annot.key, onClose])
  // Below the annotation, or above it when the page ends first; inside the page's width.
  const [x1, y1, x2, y2] = box
  const left = Math.max(8, Math.min(Math.min(x1, x2), pageW - w - 8))
  const below = Math.max(y1, y2) + 8
  const place: React.CSSProperties = below + 140 < pageH ? { top: below } : { bottom: pageH - Math.min(y1, y2) + 8 }
  return (
    <div
      ref={ref}
      role="dialog"
      aria-label={`${annot.label} by ${annot.author}`}
      tabIndex={-1}
      data-testid="pdf-annot-popover"
      data-k2-chamfer="control"
      {...floatMaterial('popover')}
      className="pdf-pop"
      style={{ left, width: w, ...place }}
      onKeyDown={(e) => {
        if (e.key !== 'Escape') return
        e.preventDefault()
        e.stopPropagation()
        onClose()
      }}
    >
      <div className="pdf-pop-head">
        <b className="type-row font-semibold">{annot.author}</b>
        <span className="flex-1" />
        <button type="button" data-k2-focus="" aria-label="Close" title="Close (Esc)" onClick={onClose} className="lv-icon-btn no-drag pdf-pop-close">
          <CloseGlyph size={13} />
        </button>
      </div>
      <div className="pdf-pop-meta type-hint">
        <span>{annot.label}</span>
        {annot.date ? (
          <>
            <span aria-hidden="true">·</span>
            <time dateTime={annot.date.toISOString()}>{formatAnnotDate(annot.date)}</time>
          </>
        ) : null}
      </div>
      {annot.text ? <p className="pdf-pop-text type-row">{annot.text}</p> : <p className="pdf-pop-text pdf-quiet type-row">No text.</p>}
    </div>
  )
}
