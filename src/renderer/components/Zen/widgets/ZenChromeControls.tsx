// prd-zen-mode-v1 Z27, prd-zen-gardens-v1 G11, G24, G25, G58 and
// prd-zen-freeform-chrome FC1, FC17, FC20, FC27 — the two required chrome
// widgets: the Garden switcher and the Zen toggle. The page places them
// from data (a band, a column edge); `ZenBands` draws them through the
// chrome registry. Inside a menu they are rows of `ZenMenu` instead.
//
// They bind through the bridge (`bridge.controls.bind`) exactly as a v2
// user page would, so K2's present / visible / keyboard / wired checks
// cover them:
//   - the Garden switcher is a pill with the Garden's name; it opens a menu
//     of every Garden (each bound as `garden-option`, with its ⌥⌘N hint), a
//     separator, then "+ New Garden" (`ZenNewGarden`), which closes the
//     menu and opens the New Garden modal (a name and the catalog's cards;
//     `ZenNewGardenModal`): Create makes the Garden (`gardens.create`, cap
//     `gardens:manage`) and switches to it. Rename and delete are CLI and
//     agent only in this cut. The menu closes on a pick, Esc or a click
//     outside. It opens toward the page (down from the top band, up from
//     the bottom band) and lines up with the pill's side, through
//     `useAnchoredMenu` (FC20), portalled into the Zen root and registered
//     as a K2 overlay. When its row is too narrow, its name truncates down
//     to 6 rem (FC27);
//   - the Zen toggle is a switch, on, that turns Zen off in this window
//     (the "Zen" label and the knob; the ensō icon is the top bar's way IN,
//     `ZenTopBarToggle`, Rosson 2026-10-04).

import { useCallback, useContext, useState } from 'react'
import type { ZenGardenSummary, ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { ZEN_GLASS_PROPS } from '@/lib/zen/zen-glass'
import { ZEN_SWITCHER_COMPACT_MAX } from '@/lib/zen/zen-overflow'
import { useZenBind, useZenK2Overlay, ZenChromePlaceContext, ZenRowCompactContext } from '../ZenTemplateControls'
import { ZenNewGarden } from './ZenNewGarden'

/** One Garden in a list (the switcher's menu, or a `menu`'s Gardens
 *  section): bound as `garden-option` with its Garden id and ⌥⌘N hint. */
export function GardenChoice({
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

/** A K2 dropdown's look: raised Zen surface, flat inside (Zen glass edge). */
export const ZEN_DROPDOWN_STYLE: React.CSSProperties = {
  padding: 4,
  gap: 2,
  background: 'var(--zen-surface-raised)',
  color: 'var(--zen-text)',
  border: '1px solid var(--zen-border)',
  borderRadius: 'var(--zen-radius)',
  boxShadow: '0 10px 30px rgba(0, 0, 0, 0.14)',
}

/** The Garden switcher (G25): a pill naming this window's Garden. */
export function ZenGardenSwitcher({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const place = useContext(ZenChromePlaceContext)
  const compact = useContext(ZenRowCompactContext)
  const trigger = useZenBind(bridge, 'garden-switcher')
  const close = useCallback(() => setOpen(false), [])
  const manage = bridge.caps.has('gardens:manage')
  const menu = useAnchoredMenu<HTMLDivElement>({
    open,
    onClose: close,
    gap: 6,
    width: 'min',
    minWidth: 240,
    prefer: place.opens,
    align: place.align === 'end' ? 'end' : 'start',
  })
  const overlay = useZenK2Overlay()
  const menuRef = useCallback(
    (el: HTMLDivElement | null) => {
      menu.menuRef(el)
      overlay(el)
    },
    [menu.menuRef, overlay],
  )
  const gardens = bridge.gardens.list()
  const current = bridge.gardens.current()

  return (
    <div ref={menu.anchorRef} className="relative" data-zen-garden-switcher="" style={{ flexShrink: 0 }}>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        title="Switch Garden"
        onClick={() => (open ? close() : setOpen(true))}
        data-zen-garden-pill=""
        data-zen-soft-button=""
        data-zen-compact={compact ? '' : undefined}
        {...ZEN_GLASS_PROPS}
        className="flex items-center gap-2 cursor-pointer"
        style={{
          // A shared Zen glass tile (`zen-glass.ts`): background, edge, blur.
          height: 30,
          minWidth: 24,
          padding: '0 12px 0 14px',
          color: 'var(--zen-text)',
          borderRadius: 999,
          fontWeight: 600,
        }}
      >
        <span className="truncate" style={{ maxWidth: compact ? ZEN_SWITCHER_COMPACT_MAX : '14rem' }}>
          {current?.name ?? 'Gardens'}
        </span>
        <svg
          width="10"
          height="10"
          viewBox="0 0 10 10"
          aria-hidden
          style={{ color: 'var(--zen-text-muted)', transform: place.opens === 'up' ? 'rotate(180deg)' : undefined }}
        >
          <path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open &&
        menu.portal(
          <div
            ref={menuRef}
            role="menu"
            data-zen-garden-menu=""
            data-zen-menu-placement={menu.placement}
            className="flex flex-col"
            style={{ ...menu.style, ...ZEN_DROPDOWN_STYLE }}
          >
            {gardens.map((g) => (
              <GardenChoice key={g.id} bridge={bridge} garden={g} selected={g.id === current?.id} onPicked={close} />
            ))}
            {manage && (
              <>
                <div role="separator" aria-hidden style={{ height: 1, margin: '3px 6px', background: 'var(--zen-border)' }} />
                <ZenNewGarden bridge={bridge} onDone={close} />
              </>
            )}
          </div>,
        )}
    </div>
  )
}

/** The Zen toggle: the way out, a switch that is on. */
export function ZenToggle({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
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
      className="flex items-center gap-2 cursor-pointer"
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
