// Title bar for ConnectionGate overlays (connecting / sign-in).
// During a host swap only the server picker stays visible so the
// user can go back to Local. Traffic-light spacer / window controls
// stay so the window remains movable and closable.

import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { Surface } from '@/components/ui'
import ServerSwitcher from './ServerSwitcher'
import DesktopChromeLeft from './DesktopChromeLeft'
import DesktopChromeRight from './DesktopChromeRight'
import K2MarkButton from './K2MarkButton'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'

export default function GateChrome(): React.JSX.Element {
  return (
    <Surface
      role2="surface"
      bordered={false}
      className="flex items-center justify-between border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
      onMouseDown={titleBarDragOnMouseDown}
      onDoubleClick={titleBarOnDoubleClick}
      style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
    >
      <div
        className="flex items-center [&>*]:shrink-0"
        style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
      >
        <DesktopChromeLeft />
        <K2MarkButton />
        <ServerSwitcher />
      </div>
      <DesktopChromeRight />
    </Surface>
  )
}
