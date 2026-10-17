import { Children, type ReactNode } from 'react'
import TimerButton from '@/components/Timer/TimerButton'
import UsageButton from '@/components/Timer/UsageButton'
import KeepAwakeButton from '@/components/Timer/KeepAwakeButton'
import HelpButton from '@/components/Help/HelpButton'
import PresenceRoster from '@/components/Presence/PresenceRoster'
import ModeToggle from '@/components/Presence/ModeToggle'
import { zenAvailable } from '@/lib/zen/zen-platform'
import { TopBarFollowRoomContext } from './top-bar-scope'
import TopBarPipe from './TopBarPipe'
import ZenTopBarToggle from './ZenTopBarToggle'
import AdminProfileButton from './AdminProfileButton'

/** True when this page passed drawer or close controls after the last pipe. */
export function hasTopBarPageToggles(children: ReactNode): boolean {
  return Children.toArray(children).length > 0
}

/**
 * Right-cluster body shared with TopBar: presence (with its own pipe, only
 * while the roster shows), usage, pipe, timer,
 * keep awake, Help, pipe, mode, then a pipe only when that page has
 * its own toggles, then (where Zen exists) a pipe and the Zen toggle as the
 * LAST item: right of the drawer toggles on Agents and Home
 * (prd-zen-gardens-v1 G5, G63). The Admin profile button sits right of
 * the Zen toggle, wherever that toggle shows (Rosson 2026-10-16).
 * No Agents run button — pass it as `leading` from TopBar only.
 *
 * 0.43.2 Z36: `followRoom` (TopBar only) lets presence, usage, Keep awake
 * and the mode toggle show a focused remote Home room's server. The timer
 * stays on the window's server. Every other page's bar leaves it off.
 */
export default function TopBarUtilities({
  leading,
  followRoom = false,
  children,
}: {
  leading?: ReactNode
  followRoom?: boolean
  children?: ReactNode
}): React.JSX.Element {
  const pageToggles = hasTopBarPageToggles(children)
  return (
    <TopBarFollowRoomContext.Provider value={followRoom}>
    <div className="flex items-center gap-1 no-drag">
      {leading}
      <PresenceRoster />
      <UsageButton />
      <TopBarPipe />
      <TimerButton />
      <KeepAwakeButton />
      <HelpButton />
      <TopBarPipe />
      <ModeToggle />
      {pageToggles ? <TopBarPipe /> : null}
      {children}
      {zenAvailable() ? (
        <>
          <TopBarPipe />
          <ZenTopBarToggle />
          <AdminProfileButton />
        </>
      ) : null}
    </div>
    </TopBarFollowRoomContext.Provider>
  )
}
