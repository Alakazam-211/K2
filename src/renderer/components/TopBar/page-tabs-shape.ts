// Which optional page tabs the top bar showed last: Mail (by login role) and
// Org Chart (the server's switch). The connecting screen has neither answer
// yet, so its top bar (GateChrome) reserves the last-seen page tabs' room and
// the server switcher stays where it was. A per-window convenience in
// localStorage, not shared state: a wrong guess costs one tab's width.

export type PageTabsShape = { mail: boolean; orgChart: boolean }

const KEY = 'k2.pageTabs.shape'

/** Before any bar has drawn: a fresh install's first login is the owner
 *  (Mail shown), and the Org Chart switch starts off. */
export const DEFAULT_PAGE_TABS_SHAPE: PageTabsShape = { mail: true, orgChart: false }

export function rememberPageTabsShape(shape: PageTabsShape): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ mail: shape.mail, orgChart: shape.orgChart }))
  } catch {
    /* storage blocked: the gate falls back to the default */
  }
}

export function lastPageTabsShape(): PageTabsShape {
  try {
    const raw = localStorage.getItem(KEY)
    if (raw) {
      const v = JSON.parse(raw) as Partial<PageTabsShape> | null
      if (v && typeof v.mail === 'boolean' && typeof v.orgChart === 'boolean') {
        return { mail: v.mail, orgChart: v.orgChart }
      }
    }
  } catch {
    /* blocked or malformed */
  }
  return { ...DEFAULT_PAGE_TABS_SHAPE }
}
