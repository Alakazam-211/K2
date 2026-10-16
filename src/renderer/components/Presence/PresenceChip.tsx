// A person chip (plan-agent-status-unify-v1 §3.5, Q8: chamfered square,
// approved 2026-10-16). One look everywhere people appear: the top-bar
// roster, the who's-connected modal, Agents and Home rows, ticket
// assignees.
//
//   - Shape: a square with 20% chamfered corners (CSS clip-path). It reads
//     as a square, like the A3 icons and the status marks, but the cut
//     corners say "person, not agent" (agent avatars are sharp squares).
//   - Fill: the person's colour, hashed from the username into 8 hues that
//     avoid the status amber, orange, green and red. White initials (one
//     letter below 20px, two at 20px and up) or the photo when one exists.
//   - Role: a small corner mark, never colour: filled = owner, hollow =
//     admin, none = member. Amber is gone from presence.
//   - You: a 1.5px ring (top bar and modal only; rows never show you).
//   - Away: greyscale at 45%.
//   - `+N`: the same shape, neutral fill, a 1px strong border.

import type { CSSProperties } from 'react'

export type PresenceRole = 'owner' | 'admin' | 'member'

/** Eight person hues (none near needs-you orange, ok green or error red). */
export const PERSON_COLORS = ['#2563eb', '#7c3aed', '#0d9488', '#db2777', '#4f46e5', '#0891b2', '#64748b', '#c026d3'] as const

/** The chamfer polygon (20% corners). */
export const CHAMFER_CLIP = 'polygon(20% 0, 80% 0, 100% 20%, 100% 80%, 80% 100%, 20% 100%, 0 80%, 0 20%)'

/** A username's colour: stable, from a string hash. */
export function personColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return PERSON_COLORS[h % PERSON_COLORS.length]
}

/** The wire `"owner"` row is shown as "Owner". */
function shownName(name: string): string {
  return name === 'owner' ? 'Owner' : name
}

/** Initials: one letter below 20px, two at 20px and up when the name has
 *  two parts (`Rosson Long` → `RL`, `anna` → `A`). */
export function chipInitials(name: string, size: number): string {
  const n = shownName(name).trim()
  const parts = n.split(/[\s._-]+/).filter(Boolean)
  if (parts.length === 0) return '?'
  if (size >= 20 && parts.length > 1) return (parts[0][0] + parts[1][0]).toUpperCase()
  return parts[0][0].toUpperCase()
}

/** The role mark's side (px). */
function markPx(size: number): number {
  return size >= 14 ? Math.max(4, Math.round(size * 0.26)) : 4
}

function fontPx(size: number): number {
  return Math.max(7, Math.round(size * (size >= 20 ? 0.4 : 0.5)))
}

export interface PresenceChipProps {
  /** Username, or the wire literal `"owner"`. */
  name: string
  role: PresenceRole
  size?: number
  /** This is you (a ring; top bar and modal only). */
  you?: boolean
  away?: boolean
  /** A photo (Connect Cloud), drawn over the person colour. */
  imageUrl?: string | null
  /** Tooltip; default "<name> · <role>". Pass '' for none (a parent titles). */
  title?: string
}

export function PresenceChip({ name, role, size = 20, you = false, away = false, imageUrl, title }: PresenceChipProps): React.JSX.Element {
  const display = shownName(name)
  const color = personColor(name)
  const mark = role === 'owner' || role === 'admin' ? markPx(size) : 0
  const tip = title ?? `${display}${you ? ' (you)' : ''} · ${role}${away ? ' · away' : ''}`
  const fill: CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: '100%',
    height: '100%',
    clipPath: CHAMFER_CLIP,
    backgroundColor: color,
    backgroundImage: imageUrl ? `url("${imageUrl}")` : undefined,
    backgroundSize: 'cover',
    backgroundPosition: 'center',
    color: '#fff',
    fontSize: fontPx(size),
    fontWeight: 700,
    lineHeight: 1,
    letterSpacing: '-0.02em',
    filter: away ? 'grayscale(1)' : undefined,
    opacity: away ? 0.45 : undefined,
  }
  return (
    <span
      className="relative inline-flex flex-none select-none"
      data-presence-chip={role}
      data-you={you ? '' : undefined}
      title={tip || undefined}
      style={{ width: size, height: size, boxSizing: 'border-box', padding: you ? 1.5 : 0 }}
    >
      {/* The ring sits under the fill and outside the fill's clip, so the
          role mark (outside every clip) stays whole. */}
      {you && (
        <span
          aria-hidden
          data-you-ring=""
          style={{ position: 'absolute', inset: 0, clipPath: CHAMFER_CLIP, background: 'var(--color-text-secondary)' }}
        />
      )}
      <span style={{ ...fill, position: 'relative' }} aria-hidden>
        {imageUrl ? null : chipInitials(name, size)}
      </span>
      {mark > 0 && (
        <i
          aria-hidden
          data-role-mark={role}
          style={{
            position: 'absolute',
            top: -1,
            right: -1,
            width: mark,
            height: mark,
            boxSizing: 'border-box',
            border: `1px solid ${role === 'owner' ? 'var(--color-bg-surface)' : 'var(--color-text-primary)'}`,
            background: role === 'owner' ? 'var(--color-text-primary)' : 'var(--color-bg-surface)',
          }}
        />
      )}
    </span>
  )
}

/** The `+N` chip: the same shape, neutral. */
export function PresenceMoreChip({ count, size = 20, title }: { count: number; size?: number; title?: string }): React.JSX.Element {
  return (
    <span
      className="relative inline-flex flex-none select-none"
      data-presence-more={count}
      title={title ?? `${count} more here`}
      style={{ width: size, height: size }}
    >
      <span
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: '100%',
          height: '100%',
          clipPath: CHAMFER_CLIP,
          background: 'var(--color-bg-elevated)',
          boxShadow: 'inset 0 0 0 1px var(--color-border-strong)',
          color: 'var(--color-text-secondary)',
          fontSize: Math.max(7, Math.round(size * (size >= 20 ? 0.4 : 0.5))),
          fontWeight: 600,
          lineHeight: 1,
        }}
      >
        +{count}
      </span>
    </span>
  )
}
