// The zoom control's middle button: today's zoom ("73%"), and a small menu
// under it with Fit page, Fit width and 100%, the current one ticked.
// Arrow keys move between them, Escape closes and puts the keyboard back on
// the button, a click elsewhere closes. It opens inside the tab, under its
// own header.

import { useEffect, useRef, useState } from 'react'
import { CheckIcon } from 'lucide-react'
import { floatMaterial } from '@/components/ui'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import type { ZoomMode } from '@/lib/pdf/zoom'

export type ZoomChoice = 'page' | 'width' | 'actual'
const ITEMS: { id: ZoomChoice; label: string }[] = [
  { id: 'page', label: 'Fit page' },
  { id: 'width', label: 'Fit width' },
  { id: 'actual', label: '100%' },
]

export function PdfZoomMenu({ pct, mode, zoom, onChoose }: { pct: string; mode: ZoomMode; zoom: number; onChoose: (c: ZoomChoice) => void }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const btn = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const on = (c: ZoomChoice): boolean => (c === 'actual' ? mode === 'custom' && Math.abs(zoom - 1) < 1e-6 : mode === c)
  useEffect(() => {
    if (!open) return
    const items = menu.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]')
    ;(menu.current?.querySelector<HTMLElement>('[aria-checked="true"]') ?? items?.[0])?.focus()
    const away = (e: MouseEvent): void => {
      const t = e.target as Node
      if (!menu.current?.contains(t) && !btn.current?.contains(t)) setOpen(false)
    }
    document.addEventListener('mousedown', away)
    return () => document.removeEventListener('mousedown', away)
  }, [open])
  const close = (): void => {
    setOpen(false)
    btn.current?.focus()
  }
  const onKey = (e: React.KeyboardEvent): void => {
    const items = [...(menu.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [])]
    const i = items.indexOf(document.activeElement as HTMLElement)
    if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault()
      e.stopPropagation()
      close()
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      items[(i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus()
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault()
      items[e.key === 'Home' ? 0 : items.length - 1]?.focus()
    }
  }
  return (
    <span className="pdf-zoom-wrap">
      <button
        ref={btn}
        type="button"
        data-k2-focus=""
        data-k2-chamfer="control"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Zoom ${pct}, choose a fit`}
        title="Fit page, Fit width or 100%"
        onClick={() => setOpen((o) => !o)}
        className="lv-icon-btn pdf-zoom-btn type-row lv-num no-drag"
      >
        {pct}
      </button>
      {open ? (
        <div ref={menu} role="menu" aria-label="Zoom" data-k2-chamfer="control" {...floatMaterial('menu')} className="pdf-zoom-menu" onKeyDown={onKey}>
          {ITEMS.map((it) => (
            <button
              key={it.id}
              type="button"
              role="menuitemradio"
              aria-checked={on(it.id)}
              tabIndex={-1}
              className="pdf-zoom-item type-row no-drag"
              onClick={() => {
                onChoose(it.id)
                close()
              }}
            >
              <span aria-hidden="true" className="pdf-zoom-tick">
                {on(it.id) ? <CheckIcon size={14} strokeWidth={K2_ICON_STROKE} /> : null}
              </span>
              {it.label}
            </button>
          ))}
        </div>
      ) : null}
    </span>
  )
}
