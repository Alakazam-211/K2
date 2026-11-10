// Presence — one person chip (plan-agent-status-unify-v1 §3.5, Q8:
// chamfered square, approved 2026-10-16). The drawing is `PresenceChip`:
// the person colour the daemon serves (else the username hash), white initials (or the photo
// once K2 Connect Cloud supplies one), and the role as a corner mark
// (filled = owner, hollow = admin, none = member). Role is never a colour,
// and amber is gone from presence.

import { PresenceChip, type PresenceRole } from './PresenceChip'
import type { PersonNames } from '@/lib/person-name'
import type { ServerScope } from '@/kessel/server-scope'

export interface PresenceAvatarProps {
  /** Username, or the wire literal `"owner"` (displayed as "Owner"). */
  name: string
  role: PresenceRole
  /** Side in px: 20 in the top bar, 14 on rows, 32 in the modal. */
  size?: number
  /** This is you: a ring (top bar and modal only; rows never show you). */
  you?: boolean
  away?: boolean
  /** A picture over the person colour; absent: their profile picture
   *  (`PresenceChip`, 0.46.8). */
  imageUrl?: string | null
  /** The server whose picture to show (`PresenceChip`). */
  pictureScope?: ServerScope | null
  title?: string
  /** The daemon's effective person colour (roster `color`). */
  color?: string | null
  /** 0.46.0: first/last name (initials, tooltip). */
  names?: PersonNames | null
}

/** The wire `"owner"` row renders as "Owner" everywhere. */
export function presenceDisplayName(name: string): string {
  return name === 'owner' ? 'Owner' : name
}

/** Who you are on the window's server, as its roster names you. */
export { useSelfPresenceUser } from './self-presence'

export default function PresenceAvatar({ name, role, size = 20, you, away, imageUrl, pictureScope, title, color, names }: PresenceAvatarProps): React.JSX.Element {
  return (
    <PresenceChip
      name={name}
      role={role}
      size={size}
      you={you}
      away={away}
      imageUrl={imageUrl}
      pictureScope={pictureScope}
      title={title}
      color={color}
      names={names}
    />
  )
}
