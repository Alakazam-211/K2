// prd-zen-mode-v1 Z1, Z10, Z27, prd-zen-gardens-v1 G11, G58 and
// prd-zen-freeform-chrome FC1, FC43 — what draws a resolved Garden page.
//
// The page is data (layout + widgets + placement). Two registries turn it
// into React:
//   - widgets by `kind` (`agents`, `conversation`, `garden-empty`). The
//     built-ins register with `registerZenWidget`; until then a placeholder
//     fills the column.
//   - chrome by `kind` (FC43): K2's controls, `garden-switcher`,
//     `zen-toggle`, `usage`, `theme-picker` and `menu`. The page places them
//     from data (`page.placement`) in a band, at a column edge or in a menu,
//     and `ZenBands` draws each through this registry. Both templates and
//     safe mode use the same entries (the per-template controls map is
//     gone). Add agent is the Agents widget's own last row, not chrome.
// Every widget gets only a `ZenWidgetBridge` built with its declared caps;
// chrome gets the template bridge (`ZEN_TEMPLATE_CONTROL_CAPS`, FC31).
// A RAIL kind (`nav-rail`) is drawn as a thin strip at the left edge of its
// column, outside the column's box, and takes no share of the box (or as a
// row in a band or at a column edge). An UNBOXED kind (`tickets-view`)
// draws its own panels, so its column has no box at all.

import { useContext, type ComponentType } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenChromeItem, ZenWidgetDecl } from '@/lib/zen/zen-page'
import { ZenGardenSwitcher, ZenToggle } from './widgets/ZenChromeControls'
import { ZenMenu } from './widgets/ZenMenu'
import { ZenThemePicker } from './ZenThemeTools'
import { ZenUsageTool } from './ZenUsageTool'
import { ZenChromePlaceContext, ZenK2ChromeContext } from './ZenTemplateControls'

export interface ZenWidgetProps {
  decl: ZenWidgetDecl
  bridge: ZenWidgetBridge
}

/** What a chrome item is drawn with. */
export interface ZenChromeProps {
  item: ZenChromeItem
  /** The template bridge: the no-cap verbs (gardens.list / current /
   *  switch, zen.exit, controls.bind, theme.get) plus `agents.add` (cap
   *  `agents:add`) and `gardens.create` (cap `gardens:manage`), granted by
   *  K2 to its own controls. */
  bridge: ZenWidgetBridge
  /** A `menu`'s items, in file order (empty for every other kind). */
  menuItems: readonly ZenChromeItem[]
}

/** Caps K2 grants its own controls. */
export const ZEN_TEMPLATE_CONTROL_CAPS: readonly string[] = ['agents:add', 'gardens:manage']

/** Placeholder until S6 registers the real widget. */
function PlaceholderWidget({ decl }: ZenWidgetProps): React.JSX.Element {
  const title =
    decl.kind === 'agents'
      ? 'Agents'
      : decl.kind === 'conversation'
        ? 'Conversation'
        : decl.kind === 'garden-empty'
          ? 'Garden'
          : decl.kind
  return (
    <div
      className="flex h-full w-full items-center justify-center"
      data-zen-widget={decl.kind}
      data-zen-widget-placeholder=""
      style={{ color: 'var(--zen-text-muted)' }}
    >
      <span style={{ fontSize: 'var(--zen-font-size)' }}>{title}</span>
    </div>
  )
}

const widgets = new Map<string, ComponentType<ZenWidgetProps>>()

/** Widget kinds drawn as a strip at the left edge of their column. */
export const ZEN_RAIL_KINDS: ReadonlySet<string> = new Set(['nav-rail'])
/** Widget kinds that draw their own panels: their column gets no box
 *  (K2's Tickets view, liquid glass). */
export const ZEN_UNBOXED_KINDS: ReadonlySet<string> = new Set(['tickets-view'])

/** S6 plug-in point: the component for widget `kind`. Returns the unregister. */
export function registerZenWidget(kind: string, component: ComponentType<ZenWidgetProps>): () => void {
  widgets.set(kind, component)
  return () => {
    if (widgets.get(kind) === component) widgets.delete(kind)
  }
}

export function zenWidgetFor(kind: string): ComponentType<ZenWidgetProps> {
  return widgets.get(kind) ?? PlaceholderWidget
}

// ── Chrome by kind (FC43) ────────────────────────────────────────────────

function SwitcherChrome({ bridge }: ZenChromeProps): React.JSX.Element {
  return <ZenGardenSwitcher bridge={bridge} />
}

function ToggleChrome({ bridge }: ZenChromeProps): React.JSX.Element {
  return <ZenToggle bridge={bridge} />
}

/** K2's usage tool; nothing in safe mode (FC22). */
function UsageChrome(): React.JSX.Element | null {
  const k2 = useContext(ZenK2ChromeContext)
  const place = useContext(ZenChromePlaceContext)
  return k2.usage ? <ZenUsageTool place={place} /> : null
}

/** K2's theme control; nothing in safe mode (FC22). */
function ThemeChrome(): React.JSX.Element | null {
  const k2 = useContext(ZenK2ChromeContext)
  const place = useContext(ZenChromePlaceContext)
  return k2.theme ? <ZenThemePicker {...k2.theme} place={place} /> : null
}

function MenuChrome({ item, bridge, menuItems }: ZenChromeProps): React.JSX.Element {
  return <ZenMenu item={item} bridge={bridge} menuItems={menuItems} />
}

const BUILTIN_CHROME: ReadonlyArray<[string, ComponentType<ZenChromeProps>]> = [
  ['garden-switcher', SwitcherChrome],
  ['zen-toggle', ToggleChrome],
  ['usage', UsageChrome],
  ['theme-picker', ThemeChrome],
  ['menu', MenuChrome],
]

const chrome = new Map<string, ComponentType<ZenChromeProps>>(BUILTIN_CHROME)

/** Plug-in point: the component for chrome `kind` (tests swap one to break
 *  a required control on purpose). Returns the unregister, which puts the
 *  previous one back. */
export function registerZenChrome(kind: string, component: ComponentType<ZenChromeProps>): () => void {
  const prev = chrome.get(kind)
  chrome.set(kind, component)
  return () => {
    if (chrome.get(kind) !== component) return
    if (prev) chrome.set(kind, prev)
    else chrome.delete(kind)
  }
}

/** The component for chrome `kind`; a kind this client doesn't know draws
 *  nothing (a required one then fails the check into safe mode). */
export function zenChromeFor(kind: string): ComponentType<ZenChromeProps> | null {
  return chrome.get(kind) ?? null
}
