// A person chip (plan-agent-status-unify-v1 §3.5, Q8: chamfered square,
// approved 2026-10-16). One look everywhere people appear: the top-bar
// roster, the who's-connected modal, Agents and Home rows, ticket
// assignees.
//
//   - Shape: a square with 20% chamfered corners (CSS clip-path). It reads
//     as a square, like the A3 icons and the status marks, but the cut
//     corners say "person, not agent" (agent avatars are sharp squares).
//   - Fill: the person's colour. The daemon serves each person's effective
//     colour (0.46.0, `k2_core::person_color`): any `#rrggbb` they picked
//     in their profile (the owner's for owner-linked logins), else the
//     username hash over the Automatic palette below. Callers pass it as
//     `color` (roster rows, Home summary people, the roster's `colors` map
//     for ticket assignees); without one (an older daemon) the chip hashes
//     the name itself with `personColor`, the same hash the daemon uses.
//     Initials (first + last name when set; one letter below 20px, two at
//     20px and up) in white or near-black, whichever reads better on the
//     fill (`chipInk`); or the photo when one exists.
//   - Role: a small corner mark, never colour: filled = owner, hollow =
//     admin, none = member. Amber is gone from presence.
//   - You: a 1.5px ring (top bar and modal only; rows never show you).
//   - Away: greyscale at 45%.
//   - `+N`: the same shape, neutral fill, a 1px strong border.

import type { CSSProperties } from 'react'
import { nameInitials, personTooltip, type PersonNames } from '@/lib/person-name'

export type PresenceRole = 'owner' | 'admin' | 'member'

/** The Automatic palette (0.46.0 refresh, Rosson 2026-10-17): eight
 *  mid-saturation hues that sit together on the dark themes and still read
 *  on light — teal, petrol, indigo, violet, orchid, rose, mauve, slate. None
 *  near needs-you orange, ok green, error red or the accent blue. The daemon's
 *  `person_color::PALETTE` is the same list in the same order (the hash
 *  must match). A person may pick any other hex; this is only the default. */
export const PERSON_COLORS = ['#3a9e96', '#3a87a8', '#6a6fc8', '#8c62c4', '#b25cb0', '#c45a86', '#9b7a95', '#64778f'] as const

/** The chamfer polygon (20% corners). */
export const CHAMFER_CLIP = 'polygon(20% 0, 80% 0, 100% 20%, 100% 80%, 80% 100%, 20% 100%, 0 80%, 0 20%)'

/** A username's colour: stable, from a string hash. */
export function personColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return PERSON_COLORS[h % PERSON_COLORS.length]
}

const HEX_RE = /^#[0-9a-f]{6}$/

/** `#RRGGBB` (case-insensitive, trimmed) → lowercase, else null. */
export function normalizeHexColor(raw: string | null | undefined): string | null {
  const c = raw?.trim().toLowerCase() ?? ''
  return HEX_RE.test(c) ? c : null
}

/** The colour to paint: the server-provided one when it is a valid
 *  `#rrggbb` (any hex since 0.46.0), else the hash. */
export function resolvePersonColor(name: string, serverColor?: string | null): string {
  return normalizeHexColor(serverColor) ?? personColor(name)
}

function channel(v: number): number {
  const s = v / 255
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
}

/** WCAG relative luminance of `#rrggbb`. */
export function relativeLuminance(hex: string): number {
  const c = normalizeHexColor(hex) ?? '#000000'
  const r = parseInt(c.slice(1, 3), 16)
  const g = parseInt(c.slice(3, 5), 16)
  const b = parseInt(c.slice(5, 7), 16)
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/** Near-black for chip initials on a light fill. */
export const CHIP_INK_DARK = '#111418'
export const CHIP_INK_LIGHT = '#ffffff'

/** White initials keep at least this contrast (WCAG large-text 3:1);
 *  below it the fill is light and the initials turn near-black. */
const MIN_WHITE_CONTRAST = 3

/** The initials colour for `hex`: white (the chip's look, and what every
 *  Automatic hue gets), or near-black once the fill is light enough that
 *  white would fall under 3:1, so any picked hex stays readable. */
export function chipInk(hex: string): string {
  const onWhite = 1.05 / (relativeLuminance(hex) + 0.05)
  return onWhite >= MIN_WHITE_CONTRAST ? CHIP_INK_LIGHT : CHIP_INK_DARK
}

/** The wire `"owner"` row is shown as "Owner". */
function shownName(name: string): string {
  return name === 'owner' ? 'Owner' : name
}

/** Initials: one letter below 20px, two at 20px and up when the name has
 *  two parts (`Rosson Long` → `RL`, `anna` → `A`). */
export function chipInitials(name: string, size: number, names?: PersonNames | null): string {
  // A real name wins: its first and last initials (one below 20px).
  const fromNames = nameInitials(names)
  if (fromNames) return size >= 20 ? fromNames : fromNames.slice(0, 1)
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
  /** The daemon's effective colour for this person; absent → the hash. */
  color?: string | null
  /** 0.46.0: the person's first/last name (initials and tooltip). */
  names?: PersonNames | null
}

export function PresenceChip({ name, role, size = 20, you = false, away = false, imageUrl, title, color: serverColor, names }: PresenceChipProps): React.JSX.Element {
  const color = resolvePersonColor(name, serverColor)
  const mark = role === 'owner' || role === 'admin' ? markPx(size) : 0
  const tip = title ?? personTooltip(name, role, names, `${you ? ' (you)' : ''}`) + (away ? ' · away' : '')
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
    color: chipInk(color),
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
      data-person-color={color}
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
        {imageUrl ? null : chipInitials(name, size, names)}
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
