import TrafficLightSpacer from './TrafficLightSpacer'
import AppMenuButton from './AppMenuButton'
import { getDesktopChrome } from '@/lib/desktop-chrome'

/** Left chrome: macOS traffic-light spacer OR Win/Linux App Menu button. */
export default function DesktopChromeLeft(): React.JSX.Element | null {
  const chrome = getDesktopChrome()
  if (chrome.trafficLightSpacer) return <TrafficLightSpacer />
  if (chrome.appMenuButton) return <AppMenuButton />
  return null
}
