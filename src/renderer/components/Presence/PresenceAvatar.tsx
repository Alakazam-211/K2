// Presence S2 — circular initials avatar with a role-colored border.
//
// Forked from the initials-fallback branch of Sidebar/ProjectAvatar but
// circular and fully self-contained (no icon fetching, no cache). V1 is
// initials-ONLY by decree (PRD §3 / decisions log 2026-07-04): profile
// images arrive later from K2 Connect Cloud accounts. The `imageUrl` prop
// is accepted NOW so the Cloud identity source drops in without reshaping
// any call site — but it is deliberately not rendered yet.

/** Role → border color, decided 2026-07-04 (PRD §10): owner=amber-gold,
 *  admin=purple, member=blue, viewer=gray. Single source of truth for
 *  every presence surface — the top-bar chips, the modal's role labels,
 *  and the future workspace-nav mini avatars (S6) all read this map. */
export const ROLE_COLORS: Record<'owner' | 'admin' | 'member' | 'viewer', string> = {
  owner: '#e2a92d', // amber-gold
  admin: '#a855f7', // purple
  member: '#3b82f6', // blue
  viewer: '#8b93a1', // gray
}

export interface PresenceAvatarProps {
  /** Username, or the wire literal `"owner"` (displayed as "Owner"). */
  name: string
  role: 'owner' | 'admin' | 'member' | 'viewer'
  /** Diameter in px. ~20 for the top bar; renders fine down to 14-16
   *  (the S6 workspace-nav size) and up to ~32 (the modal). */
  size?: number
  /** Accepted but NOT rendered in V1 — avatars are initials-only until
   *  K2 Connect Cloud accounts supply profile images.
   *  TODO(K2 Connect Cloud): render the image (initials as fallback)
   *  once Cloud identity lands — this prop is the drop-in point. */
  imageUrl?: string | null
}

/** The wire `"owner"` row renders as "Owner" everywhere. */
export function presenceDisplayName(name: string): string {
  return name === 'owner' ? 'Owner' : name
}

export default function PresenceAvatar({
  name,
  role,
  size = 20,
  // V1: intentionally unused (see prop doc above).
  imageUrl: _imageUrl,
}: PresenceAvatarProps): React.JSX.Element {
  const displayName = presenceDisplayName(name)
  const initial = displayName.charAt(0).toUpperCase()
  const borderColor = ROLE_COLORS[role] ?? ROLE_COLORS.member

  return (
    <span
      className="flex flex-shrink-0 items-center justify-center select-none"
      title={`${displayName} (${role})`}
      style={{
        width: size,
        height: size,
        borderRadius: '50%',
        border: `2px solid ${borderColor}`,
        backgroundColor: 'var(--color-bg-elevated)',
        color: 'var(--color-text-primary)',
        // Border eats 4px of the diameter — scale the glyph to the
        // inner circle so it stays legible at 14-16px.
        fontSize: Math.max(7, Math.round((size - 4) * 0.6)),
        fontWeight: 700,
        lineHeight: 1,
        boxSizing: 'border-box',
      }}
    >
      {initial}
    </span>
  )
}
