// §6.7.1 — the project-group avatar: initials + a STABLE per-group
// color (ProjectAvatar's fallback look, group-shaped). Project groups
// have no icon/color fields yet — a follow-up wave adds real icons, so
// the component already takes an optional `iconUrl` (data URL) and
// renders it exactly like ProjectAvatar does; today every caller
// passes none and gets the initials fallback. Used by the nav's
// collapsed icon rail AND the expanded rows, so both stay in lockstep
// when icons arrive.

// The workspace default-color palette's accent family — hand-picked to
// read on the dark surface; the hash keeps a group's color stable
// across sessions and clients (it derives from the canonical group id).
const GROUP_AVATAR_COLORS = [
  '#e06c75',
  '#d19a66',
  '#e5c07b',
  '#98c379',
  '#56b6c2',
  '#61afef',
  '#c678dd',
  '#be5046',
  '#4ec9b0',
  '#c586c0',
]

/** Stable per-group color: FNV-ish hash of the group id → palette. */
export function groupAvatarColor(groupId: string): string {
  let h = 0
  for (let i = 0; i < groupId.length; i++) {
    h = (h * 31 + groupId.charCodeAt(i)) >>> 0
  }
  return GROUP_AVATAR_COLORS[h % GROUP_AVATAR_COLORS.length]
}

export default function ProjectGroupAvatar({
  name,
  groupId,
  size = 28,
  iconUrl = null,
}: {
  name: string
  groupId: string
  size?: number
  /** Future seam — a real group icon (data URL) renders like
   *  ProjectAvatar's image branch; null today. */
  iconUrl?: string | null
}): React.JSX.Element {
  const color = groupAvatarColor(groupId)

  if (iconUrl) {
    return (
      <span
        className="flex-shrink-0"
        style={{
          width: size,
          height: size,
          border: `2px solid ${color}`,
          overflow: 'hidden',
          display: 'block',
        }}
      >
        <img
          src={iconUrl}
          alt={name}
          style={{
            width: '100%',
            height: '100%',
            objectFit: 'cover',
            objectPosition: 'center',
            display: 'block',
          }}
        />
      </span>
    )
  }

  return (
    <span
      className="flex items-center justify-center flex-shrink-0"
      style={{
        width: size,
        height: size,
        backgroundColor: color,
        color: '#ffffff',
        fontSize: size * 0.5,
        fontWeight: 700,
        lineHeight: 1,
        fontFamily: 'inherit',
      }}
    >
      {(name.trim()[0] ?? '?').toUpperCase()}
    </span>
  )
}
