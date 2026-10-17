// Presence — one person chip (plan-agent-status-unify-v1 §3.5, Q8:
// chamfered square, approved 2026-10-16). The drawing is `PresenceChip`:
// the person colour the daemon serves (else the username hash), white initials (or the photo
// once K2 Connect Cloud supplies one), and the role as a corner mark
// (filled = owner, hollow = admin, none = member). Role is never a colour,
// and amber is gone from presence.

import { useConnectHostStore } from '@/stores/connect-host'
import { PresenceChip, type PresenceRole } from './PresenceChip'
import type { PersonNames } from '@/lib/person-name'

export interface PresenceAvatarProps {
  /** Username, or the wire literal `"owner"` (displayed as "Owner"). */
  name: string
  role: PresenceRole
  /** Side in px: 20 in the top bar, 14 on rows, 32 in the modal. */
  size?: number
  /** This is you: a ring (top bar and modal only; rows never show you). */
  you?: boolean
  away?: boolean
  /** A profile photo (K2 Connect Cloud), drawn over the person colour. */
  imageUrl?: string | null
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

/** Who you are on the window's server, as its roster names you: `owner`
 *  on this computer, else your saved login's username. */
export function useSelfPresenceUser(): string {
  return useConnectHostStore((s) => (s.activeHost === 'local' ? 'owner' : s.activeHost.username || 'owner'))
}

export default function PresenceAvatar({ name, role, size = 20, you, away, imageUrl, title, color, names }: PresenceAvatarProps): React.JSX.Element {
  return <PresenceChip name={name} role={role} size={size} you={you} away={away} imageUrl={imageUrl} title={title} color={color} names={names} />
}
