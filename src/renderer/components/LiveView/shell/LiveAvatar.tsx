// A person on a live file: the app's shared avatar shape (`data-k2-avatar`
// with the surface chamfer: round in Glass, an octagon in Terminal, the cut
// at 28% of its side as in the mocks), their initial on their colour (the
// kit's actor colour, from the data) in whichever ink reads, black or white.
// When the view gives them a presence slot (the colour their marks take on
// the file: the scene's boxes, the editors' carets), the avatar is that
// token with the page's background as ink, so one person is one colour
// everywhere on the tab. A view can map data colours to slots for avatars
// it draws from its own data (PresenceColours). You are the accent with the
// person glyph. Away people fade; while the tab
// isn't connected, everyone greys ("as you last saw them").

import { createContext, useContext, type CSSProperties } from 'react'
import { personInk } from '@/components/HomeChat/HomeChatGlyphs'
import type { LivePerson } from '@/lib/live-view/shared/people'
import { YouGlyph } from './glyphs'

const HEX = /^#[0-9a-f]{3}([0-9a-f]{3})?$/i

/** A view's presence slot per data colour (lower-case hex), for avatars it draws from its own data. */
export const PresenceColours = createContext<ReadonlyMap<string, 1 | 2 | 3 | 4> | null>(null)

/** Each slot's token, spelled out (the Styles shield reads every variable by name). */
const PRESENCE_VAR = ['var(--color-presence-1)', 'var(--color-presence-2)', 'var(--color-presence-3)', 'var(--color-presence-4)'] as const

export function LiveAvatar({
  person,
  size = 22,
  grey = false,
}: {
  person: Pick<LivePerson, 'name' | 'kind' | 'colour' | 'away' | 'self' | 'presence'>
  size?: number
  grey?: boolean
}): React.JSX.Element {
  const self = !!person.self
  const byColour = useContext(PresenceColours)
  const slot = person.presence ?? byColour?.get(person.colour.toLowerCase())
  const token = !self && slot ? PRESENCE_VAR[slot - 1] : null
  const ink = token ? 'var(--color-bg)' : self || !HEX.test(person.colour) ? 'var(--color-on-accent)' : personInk(person.colour)
  const style = {
    '--k2-ch-cut': `${Math.round(size * 0.28)}px`,
    '--k2-ch-bw': '0px',
    width: size,
    height: size,
    fontSize: Math.round(size * 0.48),
    backgroundColor: self ? 'var(--color-accent-fill)' : (token ?? person.colour),
    color: ink,
  } as CSSProperties
  return (
    <span
      aria-hidden="true"
      data-testid="lv-avatar"
      data-k2-chamfer="surface"
      data-k2-avatar=""
      data-agent={person.kind === 'agent' ? '' : undefined}
      data-away={person.away ? '' : undefined}
      data-grey={grey ? '' : undefined}
      className="lv-avatar inline-flex flex-none select-none items-center justify-center font-semibold leading-none"
      style={style}
    >
      {self ? <YouGlyph size={Math.round(size * 0.64)} /> : (person.name.trim()[0] ?? '?').toUpperCase()}
    </span>
  )
}
