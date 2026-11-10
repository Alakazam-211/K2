// The Mail page (prd-ocmail-admin-in-k2-v1 OA5): a full page beside Home,
// Agents, Projects, Tickets and Wiki. Owners and Admins see every inbox on the
// server, hosted and linked; a Member sees only their own mailboxes. Who may
// open what, the access log and the agent refusal all live in the daemon
// (/cli/mail/reader/*); this page only draws. The reader itself is a lazy
// chunk (@k2/mail-reader), loaded the first time the page opens.

import React, { Suspense } from 'react'
import { usePageViewStore } from '@/stores/page-view'
import { useConnectHostStore } from '@/stores/connect-host'
import { titleBarDragOnMouseDown, titleBarOnContextMenu, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import K2MarkButton from '@/components/TopBar/K2MarkButton'
import TopBarUtilities from '@/components/TopBar/TopBarUtilities'
import { Surface } from '@/components/ui'
import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'

const MailReaderHost = React.lazy(() => import('./MailReaderHost'))

export default function MailPage(): React.JSX.Element | null {
  const isOpen = usePageViewStore((s) => s.page === 'mail')
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  if (!isOpen) return null
  return (
    <div data-testid="mail-page" className="fixed inset-[var(--inset-window)] z-50 flex flex-col bg-[var(--color-bg)]">
      <Surface
        role2="surface"
        data-chrome="bar"
        bordered={false}
        className="flex items-center border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        onContextMenu={titleBarOnContextMenu}
        style={{ height: TOPBAR_HEIGHT_CSS, minHeight: TOPBAR_HEIGHT_CSS }}
      >
        <div
          className="flex items-center flex-1 [&>*]:shrink-0"
          style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
        >
          <DesktopChromeLeft />
          <K2MarkButton />
          <PageTabs />
          <ServerSwitcher />
        </div>
        <DesktopChromeRight>
          <TopBarUtilities />
        </DesktopChromeRight>
      </Surface>
      <div className="flex-1 min-h-0">
        <Suspense fallback={<p className="px-4 py-6 type-row text-[var(--color-text-muted)]">Loading…</p>}>
          <MailReaderHost hostKey={hostKey} />
        </Suspense>
      </div>
      <div
        data-toast-host="mail"
        className="pointer-events-none overflow-hidden"
        style={{ position: 'absolute', top: TOPBAR_HEIGHT_CSS, right: 0, bottom: 0, left: 0, zIndex: 40 }}
      />
    </div>
  )
}
