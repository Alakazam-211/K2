// 0.45.2 polish (Rosson 2026-10-08) — Reload for a custom widget that fills
// a full-canvas Garden.
//
// A custom widget normally carries K2's own ⋯ (Reload) at its box's top
// right (UW25). When the widget IS the page (`[layout] canvas = "full"`,
// one layout column, and that column's only widget), the ⋯ floats over the
// page and looks broken. Then "Reload <name>" goes in the Garden's own
// `menu` chrome widget instead: the first menu in the top band, else the
// first in the bottom band, else the first at a column edge. With no menu
// on the page the corner ⋯ stays, so Reload is never unreachable.
//
// A runtime item only: nothing is written to the Garden file.

import { createContext } from 'react'
import { zenWidgetDisplayName } from './zen-custom-words'
import { ZEN_CUSTOM_KIND, zenWidgetInBand, type ZenResolvedPage, type ZenWidgetDecl } from './zen-page'

/** Which widget fills the page and which menu holds its Reload. */
export interface ZenFillReload {
  /** The custom widget's placement id (`decl.id`). */
  widgetId: string
  /** The `menu` chrome item that shows "Reload <name>". */
  menuId: string
  /** The widget's name as K2 shows it (cut to length). */
  name: string
}

/** K2's own built-in widgets (`k2:<name>@<n>`, the Diary) have no Reload. */
export function zenIsBuiltinCustomWidget(widget: string): boolean {
  return widget.startsWith('k2:')
}

/** The widgets that fill the layout columns' bodies: not in a band, not a
 *  row item at a column edge (what `ZenPage` draws in the columns). */
export function zenBodyWidgets(page: Pick<ZenResolvedPage, 'widgets' | 'placement'>): ZenWidgetDecl[] {
  const { placement } = page
  const rowIds = new Set<string>()
  for (const g of [placement.bands.top, placement.bands.bottom, ...placement.edges]) {
    if (g) for (const id of [...g.start, ...g.center, ...g.end]) rowIds.add(id)
  }
  return page.widgets.filter((w) => !rowIds.has(w.id) && !zenWidgetInBand(w))
}

/** The menu that holds the Reload: the first drawn `menu` in the top band,
 *  then the bottom band, then the column edges (in placement order). */
function hostMenu(page: Pick<ZenResolvedPage, 'placement'>): string | null {
  const { placement } = page
  const kinds = new Map(placement.items.map((i) => [i.id, i.kind]))
  for (const g of [placement.bands.top, placement.bands.bottom, ...placement.edges]) {
    if (!g) continue
    for (const id of [...g.start, ...g.center, ...g.end]) if (kinds.get(id) === 'menu') return id
  }
  return null
}

/** When a non-built-in custom widget fills a full-canvas page and the page
 *  has a menu: that widget and menu. Otherwise null (the corner ⋯ stays). */
export function zenFillReload(page: Pick<ZenResolvedPage, 'layout' | 'widgets' | 'placement'>): ZenFillReload | null {
  if (page.layout.canvas !== 'full' || page.layout.split.length !== 1) return null
  const body = zenBodyWidgets(page)
  if (body.length !== 1) return null
  const only = body[0]
  if (only.kind !== ZEN_CUSTOM_KIND || !only.custom || zenIsBuiltinCustomWidget(only.custom.widget)) return null
  const menuId = hostMenu(page)
  if (!menuId) return null
  return { widgetId: only.id, menuId, name: zenWidgetDisplayName(only.custom.name) }
}

/** Provided by `ZenPage`: the custom widget skips its corner ⋯ and the
 *  named menu adds "Reload <name>". Null on every other page. */
export const ZenFillReloadContext = createContext<ZenFillReload | null>(null)
