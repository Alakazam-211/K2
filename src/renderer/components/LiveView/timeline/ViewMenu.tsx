// The View menu: Lane height, Pictures on clips, While playing, each a
// labelled group of K2's radios (the arrow keys move within a group; Esc
// closes and puts focus back on View). A choice is kept for this tab, and
// the menu says so in a line at its foot. Chosen radios fill with the
// accent that reads on the menu, their mark in the menu's own colour; the
// focus ring is 2px, 2px out, in the radio's shape (an octagon in Terminal,
// round in Glass). Glass frosts it, as a floating menu. It opens on the
// side of View with more room inside the tab (up over the monitor, as a
// rule) and never past the tab: when it doesn't fit, it scrolls with soft
// ends (ScrollEdgeFrame).

import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react'
import { Button, Radio, ScrollEdgeFrame, floatMaterial } from '@/components/ui'
import { confirmLine, VIEW_GROUPS, type ViewPrefs } from '@/lib/live-view/timeline/lanes-view'
import { ChevronGlyph } from '../shell/glyphs'
import { CheckGlyph } from './glyphs'

export function ViewMenu({ prefs, onChange }: { prefs: ViewPrefs; onChange: (group: keyof ViewPrefs, value: string) => void }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [done, setDone] = useState<string | null>(null)
  const btn = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const body = useRef<HTMLDivElement>(null)
  const id = useId()
  const [room, setRoom] = useState<{ up: boolean; h: number } | null>(null)

  // Its height: all of it if the room allows, else the room (it scrolls). Read again when the confirmation line comes.
  useLayoutEffect(() => {
    const b = btn.current
    if (!open || !b) return
    const tab = b.closest('[data-testid="lv-tab"]') ?? document.body
    const br = b.getBoundingClientRect()
    const tr = tab.getBoundingClientRect()
    const above = br.top - tr.top - 12
    const below = tr.bottom - br.bottom - 12
    const natural = (body.current?.offsetHeight ?? 0) + 2
    const up = above >= natural || above >= below
    const h = Math.max(120, Math.min(natural, up ? above : below))
    setRoom((o) => (o && o.up === up && o.h === h ? o : { up, h }))
  }, [open, done])

  useEffect(() => {
    if (!open) return
    menu.current?.querySelector<HTMLInputElement>('input:checked')?.focus()
    const away = (e: MouseEvent): void => {
      const t = e.target as Node
      if (!menu.current?.contains(t) && !btn.current?.contains(t)) setOpen(false)
    }
    document.addEventListener('mousedown', away)
    return () => document.removeEventListener('mousedown', away)
  }, [open])

  const close = (): void => {
    setOpen(false)
    setDone(null)
    btn.current?.focus()
  }

  return (
    <span className="relative inline-flex">
      <Button ref={btn} variant="default" size="sm" aria-haspopup="dialog" aria-expanded={open} data-testid="lv-view-btn" className="inline-flex items-center gap-1" onClick={() => (open ? close() : setOpen(true))}>
        View
        <ChevronGlyph size={12} />
      </Button>
      {open ? (
        <div
          ref={menu}
          role="dialog"
          aria-label="View"
          data-testid="lv-view-menu"
          data-down={room && !room.up ? '' : undefined}
          style={room ? { height: room.h } : undefined}
          data-k2-chamfer="surface"
          {...floatMaterial('popover', 'frost')}
          className="lv-tl-vmenu absolute border border-[var(--color-border)] bg-[var(--color-bg)] [box-shadow:var(--shadow-3)]"
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              e.stopPropagation()
              close()
            }
          }}
        >
          <ScrollEdgeFrame className="h-full" scrollClassName="overflow-y-auto" tint="var(--lv-tl-menu)" testId="lv-view-scroll">
            <div ref={body} className="lv-tl-vbody">
              {VIEW_GROUPS.map((g) => (
                <fieldset key={g.id}>
                  <legend>{g.title}</legend>
                  {g.options.map(([v, t]) => (
                    <label key={v} className="lv-tl-vopt">
                      <span className="lv-tl-ring" data-k2-chamfer="chip" aria-hidden="true">
                        <span data-k2-chamfer="chip" />
                      </span>
                      <Radio
                        name={`${id}-${g.id}`}
                        value={v}
                        checked={prefs[g.id] === v}
                        data-testid={`lv-view-${g.id}-${v}`}
                        onChange={() => {
                          onChange(g.id, v)
                          setDone(confirmLine(g.id, v))
                        }}
                      />
                      {t}
                    </label>
                  ))}
                </fieldset>
              ))}
              {done ? (
                <div className="lv-tl-vdone" role="status" data-testid="lv-view-done">
                  <CheckGlyph size={12} />
                  <span>{done}</span>
                </div>
              ) : null}
            </div>
          </ScrollEdgeFrame>
        </div>
      ) : null}
    </span>
  )
}
