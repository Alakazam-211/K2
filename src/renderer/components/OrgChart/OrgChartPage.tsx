// The Org Chart page (prd-org-chart-v1 §6.5): a full-page overlay with its
// own top bar, like the Wiki and Tickets pages, opened from the top bar's
// Org Chart tab. It shows every agent on this server (and the agents on
// other servers this server's agents can talk to) on a canvas of orgs.
// Only while the server's Org Chart switch is on: turned off while open,
// the window goes back to Agents.

import { useCallback, useEffect, useMemo } from 'react'
import { titleBarDragOnMouseDown, titleBarOnContextMenu, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { onOrgChartChanged } from '@/stores/session-events'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import K2MarkButton from '@/components/TopBar/K2MarkButton'
import TopBarUtilities from '@/components/TopBar/TopBarUtilities'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'
import { Surface } from '@/components/ui'
import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'
import { orgApiFor, type OrgApi } from './org-api'
import { OrgChartBoard } from './OrgChartBoard'

export default function OrgChartPage({ api }: { api?: OrgApi } = {}): React.JSX.Element | null {
  const page = usePageViewStore((s) => s.page)
  const setPage = usePageViewStore((s) => s.setPage)
  const on = useSettingsStore((s) => s.orgChart) === true
  const isOpen = page === 'orgChart'
  const host = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))

  // The switch went off (here or in another window) while the page is open.
  useEffect(() => {
    if (isOpen && !on) setPage('agents')
  }, [isOpen, on, setPage])

  const scope = primaryScope()
  const client = useMemo(() => api ?? orgApiFor(scope), [api, scope, host]) // eslint-disable-line react-hooks/exhaustive-deps
  const subscribe = useCallback((fn: (revision: number) => void) => onOrgChartChanged(scope, (e) => fn(e.revision)), [scope])

  if (!isOpen || !on) return null

  return (
    <div className="fixed inset-[var(--inset-window)] z-50 flex flex-col bg-[var(--color-bg)]" data-testid="org-chart-page">
      <Surface
        role2="surface"
        data-chrome="bar"
        bordered={false}
        className="flex flex-shrink-0 select-none items-center border-b border-[var(--color-border)] px-3"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        onContextMenu={titleBarOnContextMenu}
        style={{ height: TOPBAR_HEIGHT_CSS, minHeight: TOPBAR_HEIGHT_CSS }}
      >
        <div className="flex flex-1 items-center [&>*]:shrink-0" style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}>
          <DesktopChromeLeft />
          <K2MarkButton />
          <PageTabs />
          <ServerSwitcher />
          <span className="type-name text-[var(--color-text-primary)]">Org Chart</span>
        </div>
        <DesktopChromeRight>
          <TopBarUtilities>
            <button
              type="button"
              onClick={() => setPage('agents')}
              className="no-drag flex h-6 w-6 cursor-pointer items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]"
              title="Close"
              aria-label="Close the Org Chart"
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                <line x1="2" y1="2" x2="10" y2="10" />
                <line x1="10" y1="2" x2="2" y2="10" />
              </svg>
            </button>
          </TopBarUtilities>
        </DesktopChromeRight>
      </Surface>
      {/* One board per server: a server switch starts from that server's chart. */}
      <OrgChartBoard key={host} api={client} subscribe={subscribe} />
    </div>
  )
}
