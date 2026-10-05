// prd-zen-mode-v1 Z27 and prd-zen-gardens-v1 G11, G24, G25, G58 — the
// built-in templates' own controls, one component set for both
// (`k2.texting@1`, `k2.blank@1`) and for safe mode, all in one top band:
// the Garden switcher (top left, it names the Garden you're in), the drag
// area, and the top-right cluster — K2's usage tool and theme control, then the Zen toggle
// in the top-right corner, where the top bar's Zen toggle is outside Zen
// (Rosson 2026-10-04). Add agent is no longer a page control: it is the
// last row of the Agents widget (`ZenAgentsWidget`).
//
// They are the TEMPLATE's controls, drawn and laid out by the page, and they
// bind through the bridge (`bridge.controls.bind`) exactly as a v2 user page
// would, so K2's present / visible / wired checks cover them:
//   - the Garden switcher is a pill with the Garden's name; it opens a menu
//     of every Garden (each bound as `garden-option`, with its ⌥⌘N hint), a
//     separator, then "+ New Garden" (`ZenNewGarden`): a name field, then
//     Start with the default or Start empty and ask my agent, which creates
//     the Garden (`gardens.create`, cap `gardens:manage`), switches to it
//     and closes the menu; Esc steps back. Rename and delete are CLI and
//     agent only in this cut. The menu closes on a pick, Esc or a click
//     outside;
//   - the Zen toggle is an icon switch, on, that turns Zen off in this
//     window (its icon is `ZenIcon`, chosen in `lib/zen/zen-icon.ts`; while
//     `ZEN_ICON_PREVIEW` is on it is TEMPORARILY three toggles side by
//     side, one per icon candidate, each bound as `zen-toggle`);
//   - the drag area fills the top band between the switcher and the
//     top-right cluster;
//   - K2's usage tool and theme control (`ZenK2TopRightItems`, drawn by the
//     Zen root, none in safe mode) sit immediately left of the toggle, in
//     that order.

import { useEffect, useRef, useState } from 'react'
import type { ZenGardenSummary, ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenTemplateControlsProps } from '../zen-registry'
import { TEXTING_BAR_HEIGHT_PX, useZenBind, ZenK2TopRightItems } from '../ZenTemplateControls'
import { ZenWidgetStyles } from './zen-widget-kit'
import { ZenNewGarden } from './ZenNewGarden'
import { zenToggleIcons, type ZenIconOption } from '@/lib/zen/zen-icon'
import { ZenIcon } from '../ZenIcon'

function GardenChoice({
  bridge,
  garden,
  selected,
  onPicked,
}: {
  bridge: ZenWidgetBridge
  garden: ZenGardenSummary
  selected: boolean
  onPicked(): void
}): React.JSX.Element {
  const ref = useZenBind(bridge, 'garden-option', garden.id)
  return (
    <button
      ref={ref}
      type="button"
      role="menuitemradio"
      aria-checked={selected}
      onClick={onPicked}
      data-zen-garden-option={garden.id}
      className="flex w-full items-center gap-3 text-left cursor-pointer"
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
      <span className="min-w-0 flex-1 truncate">{garden.name}</span>
      {garden.index <= 9 && (
        <span style={{ color: 'var(--zen-text-muted)', fontSize: '0.8em', fontVariantNumeric: 'tabular-nums' }}>
          ⌥⌘{garden.index}
        </span>
      )}
    </button>
  )
}

/** The Garden switcher (G25): a pill naming this window's Garden. */
export function ZenGardenSwitcher({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const trigger = useZenBind(bridge, 'garden-switcher')
  const gardens = bridge.gardens.list()
  const current = bridge.gardens.current()

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
        title="Switch Garden"
        onClick={() => setOpen((v) => !v)}
        data-zen-garden-pill=""
        data-zen-soft-button=""
        className="flex items-center gap-2 cursor-pointer"
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
        <span className="max-w-[14rem] truncate">{current?.name ?? 'Gardens'}</span>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden style={{ color: 'var(--zen-text-muted)' }}>
          <path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open && (
        <div
          role="menu"
          data-zen-garden-menu=""
          className="absolute left-0 flex flex-col"
          style={{
            top: 'calc(100% + 6px)',
            zIndex: 3,
            minWidth: 240,
            padding: 4,
            gap: 2,
            background: 'var(--zen-surface-raised)',
            border: '1px solid var(--zen-border)',
            borderRadius: 'var(--zen-radius)',
            boxShadow: '0 10px 30px rgba(0, 0, 0, 0.14)',
          }}
        >
          {gardens.map((g) => (
            <GardenChoice
              key={g.id}
              bridge={bridge}
              garden={g}
              selected={g.id === current?.id}
              onPicked={() => setOpen(false)}
            />
          ))}
          {bridge.caps.has('gardens:manage') && (
            <>
              <div role="separator" aria-hidden style={{ height: 1, margin: '3px 6px', background: 'var(--zen-border)' }} />
              <ZenNewGarden bridge={bridge} onDone={() => setOpen(false)} />
            </>
          )}
        </div>
      )}
    </div>
  )
}

function ZenToggleButton({
  bridge,
  option,
  title,
}: {
  bridge: ZenWidgetBridge
  option: ZenIconOption
  title: string
}): React.JSX.Element {
  const ref = useZenBind(bridge, 'zen-toggle')
  return (
    <button
      ref={ref}
      type="button"
      role="switch"
      aria-checked
      aria-label="Exit Zen Mode"
      title={title}
      data-zen-switch=""
      data-zen-soft-button=""
      data-zen-icon-option={option.variant}
      className="flex items-center justify-center cursor-pointer"
      style={{ width: 30, height: 30, flexShrink: 0, padding: 0, borderRadius: 999, color: 'var(--zen-text)' }}
    >
      <ZenIcon variant={option.variant} on size={18} accent="var(--zen-accent)" />
    </button>
  )
}

/** The Zen toggle: an icon switch, on, that exits Zen. While
 *  `ZEN_ICON_PREVIEW` is on (TEMPORARY), all three icon candidates side by
 *  side in a dashed group, each bound as a `zen-toggle` (the required
 *  controls check passes when any one is visible). */
function ZenToggle({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const icons = zenToggleIcons()
  if (icons.length === 1) return <ZenToggleButton bridge={bridge} option={icons[0]} title="Exit Zen Mode" />
  return (
    <div
      role="group"
      aria-label="Zen icon preview (temporary): pick one"
      data-zen-icon-preview=""
      className="flex flex-shrink-0 items-center"
      style={{ gap: 2, padding: 1, border: '1px dashed var(--zen-text-muted)', borderRadius: 999 }}
    >
      {icons.map((o) => (
        <ZenToggleButton key={o.variant} bridge={bridge} option={o} title={`${o.label}. Exit Zen Mode`} />
      ))}
    </div>
  )
}

function DragArea({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const ref = useZenBind(bridge, 'drag-region')
  return <div ref={ref} data-zen-drag="" className="min-w-0 flex-1 self-stretch" />
}

/** Both templates' top band (and safe mode's): the Garden switcher top
 *  left, the drag area, then the top-right cluster — K2's theme control
 *  immediately left of the Zen toggle, the same corner the top bar's Zen
 *  toggle sits in outside Zen (Rosson 2026-10-04). */
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
      <ZenGardenSwitcher bridge={bridge} />
      <DragArea bridge={bridge} />
      <div data-zen-top-right="" className="no-drag flex flex-shrink-0 items-center gap-2">
        <ZenK2TopRightItems />
        <ZenToggle bridge={bridge} />
      </div>
    </div>
  )
}
