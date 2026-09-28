import type { ReactNode } from 'react'
import TimerButton from '@/components/Timer/TimerButton'
import UsageButton from '@/components/Timer/UsageButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import PresenceRoster from '@/components/Presence/PresenceRoster'
import ModeToggle from '@/components/Presence/ModeToggle'

function Pipe(): React.JSX.Element {
  return <div className="w-px h-4 bg-[var(--color-border)] mx-1" />
}

/**
 * Right-cluster body shared with TopBar: presence, usage, pipe, timer,
 * cheat sheet, pipe, mode, pipe, then that page's own toggles.
 * No Agents run button — pass it as `leading` from TopBar only.
 */
export default function TopBarUtilities({
  leading,
  children,
}: {
  leading?: ReactNode
  children?: ReactNode
}): React.JSX.Element {
  return (
    <div className="flex items-center gap-1 no-drag">
      {leading}
      <PresenceRoster />
      <UsageButton />
      <Pipe />
      <TimerButton />
      <K2NounsCheatSheet />
      <Pipe />
      <ModeToggle />
      <Pipe />
      {children}
    </div>
  )
}
