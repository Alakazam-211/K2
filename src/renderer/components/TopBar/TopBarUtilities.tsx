import { Children, type ReactNode } from 'react'
import TimerButton from '@/components/Timer/TimerButton'
import UsageButton from '@/components/Timer/UsageButton'
import KeepAwakeButton from '@/components/Timer/KeepAwakeButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import PresenceRoster from '@/components/Presence/PresenceRoster'
import ModeToggle from '@/components/Presence/ModeToggle'

/** True when this page passed drawer or close controls after the last pipe. */
export function hasTopBarPageToggles(children: ReactNode): boolean {
  return Children.toArray(children).length > 0
}

function Pipe(): React.JSX.Element {
  return <div className="w-px h-4 bg-[var(--color-border)] mx-1" />
}

/**
 * Right-cluster body shared with TopBar: presence, usage, pipe, timer,
 * keep awake, cheat sheet, pipe, mode, then a pipe only when that page has
 * its own toggles.
 * No Agents run button — pass it as `leading` from TopBar only.
 */
export default function TopBarUtilities({
  leading,
  children,
}: {
  leading?: ReactNode
  children?: ReactNode
}): React.JSX.Element {
  const pageToggles = hasTopBarPageToggles(children)
  return (
    <div className="flex items-center gap-1 no-drag">
      {leading}
      <PresenceRoster />
      <UsageButton />
      <Pipe />
      <TimerButton />
      <KeepAwakeButton />
      <K2NounsCheatSheet />
      <Pipe />
      <ModeToggle />
      {pageToggles ? <Pipe /> : null}
      {children}
    </div>
  )
}
