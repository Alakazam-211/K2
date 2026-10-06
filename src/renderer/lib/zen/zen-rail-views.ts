// Rosson 2026-10-04 — what a Garden with a nav rail (Garden 1) shows for
// each rail view, as a page the Zen page runtime draws like any other.
//
//   - My Home (`home`): the Garden's own page, unchanged.
//   - Agents: the same page (layout, widgets, behaviour), but every Agents
//     widget lists THIS server's workspaces the way the app's Agents page
//     does (`source: "workspaces"`), with the focus-group dropdown in place
//     of the Home picker when focus groups are on. Its widgets get their own
//     ids (`<id>@agents`), so its selection never mixes with My Home's.
//   - Projects: one column, the rail, and K2's Projects view ("Coming soon").
//   - Tickets: one column, the rail, and K2's Tickets view (liquid glass).
//
// The page's controls (the Zen toggle, the Garden switcher, wherever the
// page placed them) stay with every view, so a view switch can't fail the
// required-controls check (prd-zen-freeform-chrome FC26 rule 8, FC48). The
// `projects-view` / `tickets-view` kinds are K2's own: the daemon's schema
// never lets a Garden file place them.

import {
  zenWidgetInBand,
  type ZenChromeItem,
  type ZenColumnEdge,
  type ZenEdge,
  type ZenPlacement,
  type ZenResolvedPage,
  type ZenWidgetDecl,
} from './zen-page'
import type { ZenRailView } from './zen-window'

/** The rail widget kind (a Garden with one gets the rail views). */
export const ZEN_NAV_RAIL_KIND = 'nav-rail'
/** K2's Projects view (Rosson: coming soon). */
export const ZEN_PROJECTS_VIEW_KIND = 'projects-view'
/** K2's Tickets view (list + item, chat only, liquid glass). */
export const ZEN_TICKETS_VIEW_KIND = 'tickets-view'
/** The id suffix the Agents view gives the page's widgets. */
export const ZEN_AGENTS_VIEW_SUFFIX = '@agents'

/** The Agents widget's `source` prop: a Home's rows (default) or this
 *  server's workspaces (the Agents view). */
export function zenAgentsSource(props: Record<string, unknown>): 'home' | 'workspaces' {
  return props.source === 'workspaces' ? 'workspaces' : 'home'
}

/** How a rail draws: its `orientation` prop (the daemon always sends it),
 *  else a row in a band and a column (strip) in a column. */
export function zenRailOrientation(w: Pick<ZenWidgetDecl, 'slot' | 'props'>): 'row' | 'column' {
  const o = w.props.orientation
  if (o === 'row' || o === 'column') return o
  return zenWidgetInBand(w) ? 'row' : 'column'
}

/** Does this page have a nav rail (so the rail views apply)? */
export function zenPageHasRail(page: ZenResolvedPage): boolean {
  return page.widgets.some((w) => w.kind === ZEN_NAV_RAIL_KIND)
}

function agentsViewPage(page: ZenResolvedPage): ZenResolvedPage {
  const renamed = new Set(page.widgets.filter((w) => w.kind === 'agents').map((w) => w.id))
  const widgets = page.widgets.map((w): ZenWidgetDecl => {
    if (w.kind === 'agents') {
      return {
        ...w,
        id: `${w.id}${ZEN_AGENTS_VIEW_SUFFIX}`,
        // One server's agents: no Home picker, no server tag.
        props: { ...w.props, source: 'workspaces', 'home-picker': false, 'server-tag': false },
      }
    }
    if (w.kind === 'conversation') {
      const follows = typeof w.props.agents === 'string' && renamed.has(w.props.agents) ? w.props.agents : null
      return {
        ...w,
        id: `${w.id}${ZEN_AGENTS_VIEW_SUFFIX}`,
        props: follows ? { ...w.props, agents: `${follows}${ZEN_AGENTS_VIEW_SUFFIX}` } : w.props,
      }
    }
    return w
  })
  return { ...page, widgets }
}

/**
 * prd-zen-freeform-chrome FC48: a one-column view keeps the page's chrome.
 * Every column-edge item (a Zen control, a row rail) moves to column 0's
 * same edge and alignment, in its original order (columns left to right);
 * bands and menus are unchanged. So a Zen toggle at column 1's bottom edge
 * is still on screen in Projects and Tickets, and a view switch never fails
 * the required-controls check.
 */
export function zenPlacementInOneColumn(placement: ZenPlacement): ZenPlacement {
  const merged = new Map<ZenEdge, ZenColumnEdge>()
  const byColumn = [...placement.edges].sort((a, b) => a.column - b.column)
  for (const e of byColumn) {
    const into = merged.get(e.edge) ?? { column: 0, edge: e.edge, start: [], center: [], end: [] }
    into.start.push(...e.start)
    into.center.push(...e.center)
    into.end.push(...e.end)
    merged.set(e.edge, into)
  }
  const edges = (['top', 'bottom'] as const).flatMap((edge) => {
    const e = merged.get(edge)
    return e ? [e] : []
  })
  const items = placement.items.map((i): ZenChromeItem => (i.column !== undefined ? { ...i, column: 0 } : i))
  return { ...placement, items, edges }
}

function singleViewPage(page: ZenResolvedPage, view: ZenWidgetDecl): ZenResolvedPage {
  const rails = page.widgets.filter((w) => w.kind === ZEN_NAV_RAIL_KIND).map((w) => ({ ...w, column: 0 }))
  return {
    ...page,
    layout: { kind: 'columns', split: [100], minWidths: [0] },
    widgets: [...rails, view],
    placement: zenPlacementInOneColumn(page.placement),
  }
}

/** The page a Garden shows for `view`. A page without a rail ignores it. */
export function zenPageForView(page: ZenResolvedPage, view: ZenRailView): ZenResolvedPage {
  if (view === 'home' || !zenPageHasRail(page)) return page
  if (view === 'agents') return agentsViewPage(page)
  if (view === 'projects') {
    return singleViewPage(page, {
      id: 'k2-projects',
      kind: ZEN_PROJECTS_VIEW_KIND,
      column: 0,
      props: {},
      // "Build a new one yourself": Ask my agent (agents on this computer,
      // a drafted message, never sent).
      caps: ['agents:read', 'thread:read', 'thread:post'],
      source: 'k2',
    })
  }
  return singleViewPage(page, {
    id: 'k2-tickets',
    kind: ZEN_TICKETS_VIEW_KIND,
    column: 0,
    props: {},
    caps: [],
    source: 'k2',
  })
}
