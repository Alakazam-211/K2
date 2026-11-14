// The pictures of Home chat: a person's avatar and a channel's glyph.
//
//   - Person: their picture, else their initials on their colour (the
//     person colour every K2 surface uses: the daemon's username hash,
//     `personColor`). The shape is an agent avatar's (`ProjectAvatar`'s
//     `data-k2-avatar`): round in Glass, the surface cut in Terminal. The
//     initials take whichever ink (near-black or white) reads better on the
//     colour, so they clear 4.5:1 on every hue.
//   - Channel: `#` for an open channel, a lock for an invite-only one (null:
//     not known yet, drawn as `#`), a line icon in the app's stroke.

import type { CSSProperties, JSX } from 'react'
import { HashIcon, LockIcon } from 'lucide-react'
import { CHIP_INK_DARK, CHIP_INK_LIGHT, chipInitials, relativeLuminance, resolvePersonColor } from '@/components/Presence/PresenceChip'
import { chamferCutFor } from '@/components/Thread/chamfer'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'

/** The ink for initials on `hex`: the one with more contrast. One of the two
 *  always clears 4.58:1 (the colour can't be far from both ends). */
export function personInk(hex: string): string {
  const l = relativeLuminance(hex)
  const onWhite = 1.05 / (l + 0.05)
  const onDark = (l + 0.05) / (relativeLuminance(CHIP_INK_DARK) + 0.05)
  return onWhite >= onDark ? CHIP_INK_LIGHT : CHIP_INK_DARK
}

export function HomeChatPersonAvatar({
  username,
  name,
  picture = null,
  size = 32,
}: {
  /** Their username on that server (their colour comes from it). */
  username: string
  /** Their shown name (the initials). */
  name: string
  picture?: string | null
  size?: number
}): JSX.Element {
  const color = resolvePersonColor(username || name)
  const shape = { '--k2-ch-cut': `${chamferCutFor(size)}px`, '--k2-ch-bw': '0px', width: size, height: size } as CSSProperties
  if (picture) {
    return (
      <span className="block flex-shrink-0" data-k2-chamfer="surface" data-k2-avatar="" data-home-chat-avatar="picture" style={shape}>
        <img src={picture} alt="" className="block h-full w-full object-cover" />
      </span>
    )
  }
  return (
    <span
      aria-hidden="true"
      className="flex flex-shrink-0 items-center justify-center font-bold leading-none"
      data-k2-chamfer="surface"
      data-k2-avatar=""
      data-home-chat-avatar="initials"
      style={{ ...shape, backgroundColor: color, color: personInk(color), fontSize: Math.round(size * (size >= 20 ? 0.4 : 0.5)) }}
    >
      {chipInitials(name, size)}
    </span>
  )
}

/** A channel's glyph: `#` open, a lock invite-only. */
export function HomeChatChannelGlyph({ inviteOnly, size = 14 }: { inviteOnly: boolean | null; size?: number }): JSX.Element {
  const Icon = inviteOnly ? LockIcon : HashIcon
  return (
    <Icon
      size={size}
      strokeWidth={K2_ICON_STROKE}
      aria-hidden="true"
      data-channel-glyph={inviteOnly ? 'lock' : 'hash'}
      className="flex-shrink-0"
    />
  )
}

/** Its words, for a label: "invite-only channel" or "channel". */
export function channelKindLabel(inviteOnly: boolean | null): string {
  return inviteOnly ? 'invite-only channel' : 'channel'
}
