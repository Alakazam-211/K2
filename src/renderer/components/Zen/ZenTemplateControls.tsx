// prd-zen-mode-v1 Z27 — `k2.texting@1`'s own required controls: a Home pill
// dropdown top left (after the stoplight safe area) with ⌥⌘N hints, a drag
// strip in the middle, and a small Zen switch top right. They are drawn by
// the TEMPLATE, not by K2's shell: they bind through the bridge exactly as
// a v2 user page would, so the same check covers them. S6 restyles them
// (`registerZenTemplateControls`); the binding stays.

import { useCallback, useState } from 'react'
import type { ZenBindKind } from '@/lib/zen/zen-controls'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenTemplateControlsProps } from './zen-registry'

/** Ref callback that binds the element as `kind` (React 19 ref cleanup
 *  unbinds it). */
export function useZenBind(
  bridge: ZenWidgetBridge,
  kind: ZenBindKind,
  homeId?: string,
): (el: HTMLElement | null) => (() => void) | undefined {
  return useCallback(
    (el: HTMLElement | null) => {
      if (!el) return undefined
      return bridge.controls.bind(kind, el, homeId)
    },
    [bridge, kind, homeId],
  )
}

/** Height of the template's top band (CSS px). */
export const TEXTING_BAR_HEIGHT_PX = 44

function HomeOption({
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
  onPicked: () => void
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
      className="flex w-full items-center justify-between gap-4 px-3 py-1.5 text-left"
      style={{
        minHeight: 28,
        color: 'var(--zen-text)',
        background: selected ? 'var(--zen-surface-raised)' : 'transparent',
        borderRadius: 'calc(var(--zen-radius) / 2)',
      }}
    >
      <span className="truncate">{name}</span>
      {index < 9 && <span style={{ color: 'var(--zen-text-muted)', fontSize: '0.85em' }}>⌥⌘{index + 1}</span>}
    </button>
  )
}

function HomePill({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const trigger = useZenBind(bridge, 'home-switcher')
  const homes = bridge.homes.list()
  const selectedId = bridge.homes.selected()
  const current = homes.find((h) => h.id === selectedId) ?? homes[0]
  return (
    <div className="relative" style={{ flexShrink: 0 }}>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        data-zen-home-pill=""
        className="flex items-center gap-1.5 px-3"
        style={{
          height: 28,
          minWidth: 24,
          color: 'var(--zen-text)',
          background: 'var(--zen-surface)',
          border: '1px solid var(--zen-border)',
          borderRadius: 999,
        }}
      >
        <span className="max-w-[12rem] truncate">{current?.name ?? 'Home'}</span>
        <span aria-hidden style={{ color: 'var(--zen-text-muted)' }}>▾</span>
      </button>
      {open && (
        <div
          role="menu"
          className="absolute left-0 mt-1 min-w-[200px] p-1"
          style={{
            top: '100%',
            zIndex: 2,
            background: 'var(--zen-surface-raised)',
            border: '1px solid var(--zen-border)',
            borderRadius: 'var(--zen-radius)',
          }}
        >
          {homes.map((h, i) => (
            <HomeOption
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

function ZenSwitch({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
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
      className="flex items-center gap-1.5 px-2.5"
      style={{
        height: 28,
        minWidth: 24,
        flexShrink: 0,
        color: 'var(--zen-accent-text)',
        background: 'var(--zen-accent)',
        borderRadius: 999,
      }}
    >
      <span>Zen</span>
    </button>
  )
}

function DragStrip({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const ref = useZenBind(bridge, 'drag-region')
  return <div ref={ref} data-zen-drag="" className="min-w-0 flex-1 self-stretch" />
}

/** The texting template's top band. */
export function TextingTemplateControls({ bridge }: ZenTemplateControlsProps): React.JSX.Element {
  return (
    <div
      data-zen-template-bar=""
      className="flex flex-shrink-0 items-center gap-3"
      style={{
        height: `max(${TEXTING_BAR_HEIGHT_PX}px, var(--zen-stoplight-safe-top, 0px))`,
        paddingLeft: 'var(--zen-stoplight-safe-left, 12px)',
        paddingRight: 'calc(var(--zen-stoplight-safe-right, 0px) + 12px)',
      }}
    >
      <HomePill bridge={bridge} />
      <DragStrip bridge={bridge} />
      <ZenSwitch bridge={bridge} />
    </div>
  )
}
