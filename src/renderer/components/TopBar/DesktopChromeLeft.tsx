import TrafficLightSpacer from './TrafficLightSpacer'
import LinuxStoplights from './LinuxStoplights'
import { getDesktopChrome } from '@/lib/desktop-chrome'

/**
 * Left chrome: macOS traffic-light spacer, or Linux square stoplights.
 * Do not turn the spacer on for Linux — this returns only the spacer
 * when that flag is set, which would be an empty gap. The logo, not
 * this slot, opens the Win/Linux menu.
 */
export default function DesktopChromeLeft(): React.JSX.Element | null {
  const chrome = getDesktopChrome()
  if (chrome.trafficLightSpacer) return <TrafficLightSpacer />
  if (chrome.linuxStoplights) return <LinuxStoplights />
  return null
}
