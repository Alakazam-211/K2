// prd-zen-mode-v1 Z27 — `k2.texting@1`'s own Home switcher, Zen toggle and
// drag area, registered with `registerZenTemplateControls`. The top band
// holds the Home switcher (top left, it names the Home you're in) and the
// drag area; the footer under the Agents column holds the Zen toggle and
// the Add agent button (bottom left, Rosson 2026-10-04).
//
// They are the TEMPLATE's controls, drawn and laid out by the page, and they
// bind through the bridge (`bridge.controls.bind`) exactly as a v2 user page
// would, so K2's present / visible / wired checks cover them:
//   - the Home switcher is a pill with the Home's name; it opens a menu of
//     every Home (each bound as `home-option`, with its ⌥⌘N hint) and closes
//     on a pick, Esc or a click outside;
//   - the Zen toggle is a switch, on, that turns Zen off for this Home;
//   - the drag area fills the top band after the switcher;
//   - Add agent opens K2's Add agent picker for this Home (`agents.add`,
//     the regular Home's searchable picker) above the button.

import { useEffect, useRef, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenTemplateControlsProps } from '../zen-registry'
import { TEXTING_BAR_HEIGHT_PX, TEXTING_FOOTER_HEIGHT_PX, useZenAddAgentClick, useZenBind } from '../ZenTemplateControls'
import { ZenWidgetStyles } from './zen-widget-kit'

function HomeChoice({
  bridge,
  id,
  name,
  index,
  selected,
  onPicked,
}: {
  bridge: ZenWidgetBridge
  id: string
  name: string
  index: number
  selected: boolean
  onPicked(): void
}): React.JSX.Element {
  const ref = useZenBind(bridge, 'home-option', id)
  return (
    <button
      ref={ref}
      type="button"
      role="menuitemradio"
      aria-checked={selected}
      onClick={onPicked}
      data-zen-home-option={id}
      className="flex w-full items-center gap-3 text-left"
      style={{
        minHeight: 32,
        padding: '6px 10px',
        borderRadius: 'calc(var(--zen-radius) - 4px)',
        color: 'var(--zen-text)',
        background: selected ? 'var(--zen-surface)' : 'transparent',
        fontWeight: selected ? 600 : 400,
      }}
    >
      <span
        aria-hidden
        style={{ width: 8, height: 8, borderRadius: 999, background: selected ? 'var(--zen-accent)' : 'transparent', flexShrink: 0 }}
      />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      {index < 9 && (
        <span style={{ color: 'var(--zen-text-muted)', fontSize: '0.8em', fontVariantNumeric: 'tabular-nums' }}>
          ⌥⌘{index + 1}
        </span>
      )}
    </button>
  )
}

function HomeSwitcher({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const trigger = useZenBind(bridge, 'home-switcher')
  const homes = bridge.homes.list()
  const selectedId = bridge.homes.selected()
  const current = homes.find((h) => h.id === selectedId) ?? homes[0]

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (boxRef.current && e.target instanceof Node && !boxRef.current.contains(e.target)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div ref={boxRef} className="relative" style={{ flexShrink: 0 }}>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        title="Switch Home"
        onClick={() => setOpen((v) => !v)}
        data-zen-home-pill=""
        data-zen-soft-button=""
        className="flex items-center gap-2"
        style={{
          height: 30,
          minWidth: 24,
          padding: '0 12px 0 14px',
          color: 'var(--zen-text)',
          background: 'var(--zen-surface)',
          border: '1px solid var(--zen-border)',
          borderRadius: 999,
          fontWeight: 600,
        }}
      >
        <span className="max-w-[14rem] truncate">{current?.name ?? 'Home'}</span>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden style={{ color: 'var(--zen-text-muted)' }}>
          <path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open && (
        <div
          role="menu"
          data-zen-home-menu=""
          className="absolute left-0 flex flex-col"
          style={{
            top: 'calc(100% + 6px)',
            zIndex: 3,
            minWidth: 220,
            padding: 4,
            gap: 2,
            background: 'var(--zen-surface-raised)',
            border: '1px solid var(--zen-border)',
            borderRadius: 'var(--zen-radius)',
            boxShadow: '0 10px 30px rgba(0, 0, 0, 0.14)',
          }}
        >
          {homes.map((h, i) => (
            <HomeChoice
              key={h.id}
              bridge={bridge}
              id={h.id}
              name={h.name}
              index={i}
              selected={h.id === selectedId}
              onPicked={() => setOpen(false)}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function ZenToggle({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const ref = useZenBind(bridge, 'zen-toggle')
  return (
    <button
      ref={ref}
      type="button"
      role="switch"
      aria-checked
      aria-label="Exit Zen Mode"
      title="Exit Zen Mode"
      data-zen-switch=""
      data-zen-soft-button=""
      className="flex items-center gap-2"
      style={{ height: 30, minWidth: 24, flexShrink: 0, padding: '0 6px 0 12px', borderRadius: 999, color: 'var(--zen-text)' }}
    >
      <span style={{ fontWeight: 600, fontSize: '0.9em' }}>Zen</span>
      <span
        aria-hidden
        className="relative inline-block"
        style={{ width: 34, height: 20, borderRadius: 999, background: 'var(--zen-accent)' }}
      >
        <span
          className="absolute"
          style={{ top: 2, right: 2, width: 16, height: 16, borderRadius: 999, background: 'var(--zen-accent-text)' }}
        />
      </span>
    </button>
  )
}

function AddAgentButton({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const { ref, onClick } = useZenAddAgentClick(bridge)
  return (
    <button
      ref={ref}
      type="button"
      aria-haspopup="dialog"
      title="Add an agent to this Home"
      onClick={onClick}
      data-zen-add-agent=""
      data-zen-soft-button=""
      className="no-drag flex items-center gap-1.5"
      style={{
        height: 30,
        flexShrink: 0,
        padding: '0 12px 0 10px',
        borderRadius: 999,
        color: 'var(--zen-text)',
        border: '1px solid var(--zen-border)',
        background: 'var(--zen-surface)',
      }}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden style={{ color: 'var(--zen-text-muted)' }}>
        <path d="M6 1.5v9M1.5 6h9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
      </svg>
      <span style={{ fontWeight: 600, fontSize: '0.9em' }}>Add agent</span>
    </button>
  )
}

function DragArea({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const ref = useZenBind(bridge, 'drag-region')
  return <div ref={ref} data-zen-drag="" className="min-w-0 flex-1 self-stretch" />
}

/** The texting template's top band. */
export function ZenTextingControls({ bridge }: ZenTemplateControlsProps): React.JSX.Element {
  return (
    <div
      data-zen-template-bar=""
      data-zen-texting-controls=""
      className="flex flex-shrink-0 items-center gap-3"
      style={{
        height: `max(${TEXTING_BAR_HEIGHT_PX + 8}px, var(--zen-stoplight-safe-top, 0px))`,
        paddingLeft: 'var(--zen-stoplight-safe-left, 14px)',
        paddingRight: 'calc(var(--zen-stoplight-safe-right, 0px) + 14px)',
      }}
    >
      <ZenWidgetStyles />
      <HomeSwitcher bridge={bridge} />
      <DragArea bridge={bridge} />
    </div>
  )
}

/** The texting template's footer under the Agents column: the Zen toggle
 *  (the way out) and Add agent, in the bottom-left corner. */
export function ZenTextingFooter({ bridge }: ZenTemplateControlsProps): React.JSX.Element {
  return (
    <div
      data-zen-template-footer=""
      data-zen-texting-footer=""
      className="flex flex-shrink-0 items-center gap-2"
      style={{
        height: `calc(${TEXTING_FOOTER_HEIGHT_PX}px + var(--zen-gap, 0px))`,
        paddingTop: 'var(--zen-gap, 0px)',
      }}
    >
      <ZenToggle bridge={bridge} />
      <AddAgentButton bridge={bridge} />
    </div>
  )
}
