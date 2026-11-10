import { Children, type ReactNode } from 'react'
import TimerButton from '@/components/Timer/TimerButton'
import UsageButton from '@/components/Timer/UsageButton'
import KeepAwakeButton from '@/components/Timer/KeepAwakeButton'
import HelpButton from '@/components/Help/HelpButton'
import SendFeedbackButton from '@/components/SendFeedback/SendFeedbackButton'
import PresenceRoster from '@/components/Presence/PresenceRoster'
import VoiceButton from '@/components/Voice/VoiceButton'
import { zenAvailable } from '@/lib/zen/zen-platform'
import { TopBarFollowRoomContext } from './top-bar-scope'
import TopBarPipe from './TopBarPipe'
import ZenTopBarToggle from './ZenTopBarToggle'
import AdminProfileButton from './AdminProfileButton'
import { isWebClient } from '@/lib/is-web'

/** True when this page passed drawer or close controls after the last pipe. */
export function hasTopBarPageToggles(children: ReactNode): boolean {
  return Children.toArray(children).length > 0
}

/**
 * Right-cluster body shared with TopBar: presence (with its own pipe, only
 * while the roster shows), usage, pipe, timer,
 * keep awake, Help, Send feedback to K2, then a pipe only when that page has
 * its own toggles, then (where Zen exists) a pipe and the Zen toggle as the
 * LAST item: right of the drawer toggles on Agents and Home
 * (prd-zen-gardens-v1 G5, G63). The Admin profile button sits right of
 * the Zen toggle, wherever that toggle shows (Rosson 2026-10-16).
 * No Agents run button — pass it as `leading` from TopBar only.
 *
 * 0.43.2 Z36: `followRoom` (TopBar only) lets presence, usage and Keep
 * awake show a focused remote Home room's server. The timer stays on the
 * window's server. Every other page's bar leaves it off.
 *
 * 0.46.11: the eye/pencil left this bar for the terminal's own bar
 * (`ModeToggle` in the session header), so Help is followed directly by
 * the next group's pipe and no divider is left beside a gap.
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
    <div data-material="frost" data-frost-shape="pill" data-top-bar-cluster="" className="flex items-center gap-1 no-drag">
      {leading}
      {/* Voice rooms §4.1: avatars, headset, divider. The headset never
          follows a focused room (the call belongs to the app). */}
      <PresenceRoster trailing={({ withPipe }) => <VoiceButton withPipe={withPipe} />} />
      <UsageButton />
      <TopBarPipe />
      <TimerButton />
      <KeepAwakeButton />
      <HelpButton />
      {/* prd-k2-product-feedback-v1 PF3: right of Help. */}
      <SendFeedbackButton />
      {pageToggles ? <TopBarPipe /> : null}
      {children}
      {zenAvailable() || isWebClient() ? <TopBarPipe /> : null}
      {zenAvailable() ? <ZenTopBarToggle /> : null}
      {/* The person's own details on this server: the web client too, where
          Zen isn't. Still not in Focus or ticket windows. */}
      {zenAvailable() || isWebClient() ? <AdminProfileButton /> : null}
    </div>
    </TopBarFollowRoomContext.Provider>
  )
}
