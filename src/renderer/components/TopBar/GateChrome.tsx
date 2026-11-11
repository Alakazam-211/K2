// Title bar for ConnectionGate overlays (connecting / sign-in).
// During a host swap only the server picker stays visible so the
// user can go back to Local. Traffic-light spacer / window controls
// stay so the window remains movable and closable.
//
// The bar is laid out like the connected one (TopBar), so the switcher
// does not move when the app arrives: the same window inset around the
// bar (Layout's `p-[var(--inset-window)]`, 8px in Glass, 0 in Terminal)
// and the page tabs' room, held by a hidden footprint of the tabs the
// bar last showed.

import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'
import { titleBarDragOnMouseDown, titleBarOnContextMenu, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { Surface } from '@/components/ui'
import ServerSwitcher from './ServerSwitcher'
import DesktopChromeLeft from './DesktopChromeLeft'
import DesktopChromeRight from './DesktopChromeRight'
import K2MarkButton from './K2MarkButton'
import { PageTabsFootprint } from './PageTabs'
import { lastPageTabsShape } from './page-tabs-shape'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'

export default function GateChrome(): React.JSX.Element {
  return (
    <Surface
      role2="surface" data-chrome="bar"
      bordered={false}
      className="flex items-center justify-between border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
      onMouseDown={titleBarDragOnMouseDown}
      onDoubleClick={titleBarOnDoubleClick}
      onContextMenu={titleBarOnContextMenu}
      style={{
        height: TOPBAR_HEIGHT_CSS,
        minHeight: TOPBAR_HEIGHT_CSS,
        // Longhands, so Glass's bar margin-bottom (the pane gap) still applies.
        marginTop: 'var(--inset-window)',
        marginLeft: 'var(--inset-window)',
        marginRight: 'var(--inset-window)',
      }}
    >
      <div
        className="flex items-center [&>*]:shrink-0"
        style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
      >
        <DesktopChromeLeft />
        <K2MarkButton />
        <PageTabsFootprint shape={lastPageTabsShape()} />
        <ServerSwitcher />
      </div>
      <DesktopChromeRight />
    </Surface>
  )
}
