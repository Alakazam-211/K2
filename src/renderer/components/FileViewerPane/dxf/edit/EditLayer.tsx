// What editing draws over the drawing:
//   - EditMarks, inside the drawing's SVG in drawing coordinates: the
//     selection, each entity's box in your own accent; agents' holds, dashed
//     in the holder's presence colour;
//   - the ghost a drag moves (copies of the selected shapes, faint, with
//     their boxes) and the selection box (solid for a window, dashed for a
//     crossing), both driven straight in the DOM;
//   - HoldAsk, the one question before an edit over an agent's hold.

import { Button, floatMaterial } from '@/components/ui'
import { PRESENCE_TOKEN, type PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { ModelRow } from '../model/types'
import type { EditPrompt } from './useCadEdit'
import type { Ghost, SelectBox } from './sheet-tool'

const HAIR = 'non-scaling-stroke'
const SVG_NS = 'http://www.w3.org/2000/svg'
export const ACCENT = 'var(--color-accent)'

type Box4 = [number, number, number, number]
const padded = (b: Box4, pad: number) => ({ x: b[0] - pad, y: b[1] - pad, width: b[2] - b[0] + 2 * pad, height: b[3] - b[1] + 2 * pad })

export function EditMarks(p: {
  selection: readonly string[]
  byHandle: ReadonlyMap<string, ModelRow>
  /** Held handles, each with its holder's pid. */
  held: ReadonlyMap<string, string>
  slots: ReadonlyMap<string, PresenceSlot>
  pad: number
}): React.JSX.Element {
  return (
    <g transform="scale(1 -1)" pointerEvents="none" data-testid="dxf-edit-marks">
      {[...p.held].map(([h, pid]) => {
        const b = p.byHandle.get(h)?.bbox
        const slot = p.slots.get(pid)
        if (!b) return null
        const c = slot ? PRESENCE_TOKEN[slot] : 'var(--color-text-secondary)'
        return (
          <rect key={`h${h}`} data-testid="dxf-hold-mark" data-h={h} data-by={pid} {...padded(b, p.pad * 2)} fill="none" stroke={c} strokeOpacity={0.85} strokeWidth={1} strokeDasharray="4 3" vectorEffect={HAIR} />
        )
      })}
      {p.selection.map((h) => {
        const b = p.byHandle.get(h)?.bbox
        if (!b) return null
        return <rect key={`s${h}`} data-testid="dxf-selected" data-h={h} {...padded(b, p.pad)} fill={ACCENT} fillOpacity={0.08} stroke={ACCENT} strokeWidth={1.5} vectorEffect={HAIR} />
      })}
    </g>
  )
}

/** The ghost: copies of the selected shapes, drawn faint and moved with the pointer. */
export function domGhost(rootOf: () => SVGGElement | null, byHandle: () => ReadonlyMap<string, ModelRow>, pad: () => number, reduced: () => boolean): Ghost {
  let inner: SVGGElement | null = null
  let timer: ReturnType<typeof setTimeout> | undefined
  const clear = () => {
    clearTimeout(timer)
    inner?.remove()
    inner = null
  }
  return {
    show(handles) {
      clear()
      const root = rootOf()
      if (!root) return
      const svg = root.ownerSVGElement
      inner = document.createElementNS(SVG_NS, 'g')
      inner.setAttribute('data-testid', 'dxf-ghost')
      inner.style.opacity = '0.6'
      const want = new Set(handles)
      const shapes = svg?.querySelector('[data-testid="dxf-shapes"]')
      for (const el of shapes ? Array.from(shapes.querySelectorAll('[data-h]')) : []) {
        const h = el.getAttribute('data-h')!
        if (!want.has(h) || el.closest('defs') || el.parentElement?.closest(`[data-h="${h}"]`)) continue
        const copy = el.cloneNode(true) as SVGElement
        copy.removeAttribute('data-h')
        inner.appendChild(copy)
      }
      const rows = byHandle()
      for (const h of handles) {
        const b = rows.get(h)?.bbox
        if (!b) continue
        const r = document.createElementNS(SVG_NS, 'rect')
        const q = padded(b, pad())
        for (const [k, v] of Object.entries(q)) r.setAttribute(k, String(v))
        r.setAttribute('fill', 'none')
        r.setAttribute('stroke', ACCENT)
        r.setAttribute('stroke-dasharray', '4 3')
        r.setAttribute('vector-effect', HAIR)
        inner.appendChild(r)
      }
      root.appendChild(inner)
    },
    at(dx, dy) {
      if (!inner) return
      inner.style.transition = ''
      inner.style.transform = `translate(${dx}px, ${dy}px)`
      inner.setAttribute('data-d', `${dx} ${dy}`)
    },
    snapBack() {
      if (!inner) return
      if (reduced()) return clear()
      inner.style.transition = 'transform var(--motion-duration-fast) var(--motion-ease-standard)'
      inner.style.transform = 'translate(0px, 0px)'
      inner.setAttribute('data-d', '0 0')
      timer = setTimeout(clear, 220)
    },
    clear,
  }
}

/** The selection box, in screen pixels over the sheet. */
export function domBox(el: HTMLDivElement | null): SelectBox {
  return {
    show(a, b, mode) {
      if (!el) return
      const r = el.parentElement!.getBoundingClientRect()
      el.style.display = 'block'
      el.style.left = `${Math.min(a.x, b.x) - r.left}px`
      el.style.top = `${Math.min(a.y, b.y) - r.top}px`
      el.style.width = `${Math.abs(b.x - a.x)}px`
      el.style.height = `${Math.abs(b.y - a.y)}px`
      el.style.borderStyle = mode === 'window' ? 'solid' : 'dashed'
      el.setAttribute('data-mode', mode)
    },
    clear() {
      if (!el) return
      el.style.display = 'none'
      el.removeAttribute('data-mode')
    },
  }
}

export function SelectBoxEl({ boxRef }: { boxRef: React.Ref<HTMLDivElement> }): React.JSX.Element {
  return (
    <div
      ref={boxRef}
      aria-hidden="true"
      data-testid="dxf-select-box"
      className="pointer-events-none absolute border border-[var(--color-accent)] bg-[color-mix(in_srgb,var(--color-accent)_10%,transparent)]"
      style={{ display: 'none' }}
    />
  )
}

/** The question before an edit over an agent's hold. */
export function HoldAsk({ prompt }: { prompt: EditPrompt }): React.JSX.Element {
  return (
    <div
      {...floatMaterial('popover')}
      role="alertdialog"
      aria-label="Someone is working on these"
      aria-describedby="dxf-hold-ask-words"
      data-testid="dxf-hold-ask"
      className="absolute left-1/2 top-3 z-10 w-[min(360px,calc(100%-24px))] -translate-x-1/2 rounded-[var(--radius-box)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-3 py-2.5 text-[var(--color-text-primary)] shadow-[var(--shadow-3)]"
    >
      <p id="dxf-hold-ask-words" className="type-row m-0">
        {prompt.text}
      </p>
      <div className="mt-2 flex justify-end gap-2">
        <Button size="sm" variant="quiet" onClick={prompt.no} data-testid="dxf-hold-ask-no">
          Cancel
        </Button>
        <Button size="sm" variant="accent" onClick={prompt.yes} data-testid="dxf-hold-ask-yes" autoFocus>
          {prompt.verb === 'move' ? 'Move anyway' : 'Change anyway'}
        </Button>
      </div>
    </div>
  )
}
