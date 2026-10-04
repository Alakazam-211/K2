import { Children, type ReactNode } from 'react'
import TimerButton from '@/components/Timer/TimerButton'
import UsageButton from '@/components/Timer/UsageButton'
import KeepAwakeButton from '@/components/Timer/KeepAwakeButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import PresenceRoster from '@/components/Presence/PresenceRoster'
import ModeToggle from '@/components/Presence/ModeToggle'
import { TopBarFollowRoomContext } from './top-bar-scope'
import TopBarPipe from './TopBarPipe'

/** True when this page passed drawer or close controls after the last pipe. */
export function hasTopBarPageToggles(children: ReactNode): boolean {
  return Children.toArray(children).length > 0
}

/**
 * Right-cluster body shared with TopBar: presence, usage, pipe, timer,
 * keep awake, cheat sheet, pipe, mode, then a pipe only when that page has
 * its own toggles.
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
      <K2NounsCheatSheet />
      <TopBarPipe />
      <ModeToggle />
      {pageToggles ? <TopBarPipe /> : null}
      {children}
    </div>
    </TopBarFollowRoomContext.Provider>
  )
}
