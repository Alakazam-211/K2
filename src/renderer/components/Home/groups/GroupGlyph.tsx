// One Home group's mark: its Lucide icon, drawn squared like K2's own icons
// (style A3, `components/icons/K2Icons.tsx`: square caps, mitred joins,
// currentColor), or its position number when it has none or its name isn't
// in this build's list. The number is the group's position (its ⌥⌘N).
//
// Two sizes of "no icon" (K2 Design's notes): where the glyph is small (the
// bar's 8 px, the dialog list's 12 px) the bare numeral, bold; where there
// is room (the dialog's "No icon" choice) the outlined dot
// with the number in it: Terminal's cut octagon, Glass's circle.

import { groupIcon, type GroupIconDef } from '@/lib/group-icons'
import { K2_ICON_PX } from '@/components/icons/K2Icons'
import { groupGlyphStroke, groupNumeralPx } from './group-bar-look'

export { GROUP_BAR_ICON_PX } from './group-bar-look'

/** A curated icon in the A3 look; the stroke follows the size
 *  (`groupGlyphStroke`) unless given. */
export function GroupIconSvg({ def, size = K2_ICON_PX, strokeWidth }: { def: GroupIconDef; size?: number; strokeWidth?: number }): React.JSX.Element {
  const Icon = def.Icon
  return (
    <Icon
      size={size}
      strokeWidth={strokeWidth ?? groupGlyphStroke(size)}
      strokeLinecap="square"
      strokeLinejoin="miter"
      color="currentColor"
      aria-hidden="true"
      focusable="false"
      data-group-icon={def.name}
      className="block flex-shrink-0"
    />
  )
}

/** The bare number for a group with no icon, sized to sit with `size` px
 *  glyphs (9 px bold beside 8 px). */
export function GroupNumeral({ n, size }: { n: number; size: number }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      data-group-dot={n}
      data-group-numeral=""
      className="block font-bold leading-none tabular-nums"
      style={{ fontSize: groupNumeralPx(size) }}
    >
      {n}
    </span>
  )
}

/** The outlined dot with the number in it (1-based position), full size:
 *  the cut octagon in Terminal, the circle elsewhere (home-groups.css). */
export function GroupDot({ n, size = K2_ICON_PX }: { n: number; size?: number }): React.JSX.Element {
  const two = String(n).length > 1
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinejoin="miter"
      aria-hidden="true"
      focusable="false"
      data-group-dot={n}
      className="block flex-shrink-0"
    >
      <path data-group-dot-shape="cut" d="M7 3h10l4 4v10l-4 4H7l-4-4V7z" />
      <circle data-group-dot-shape="round" cx="12" cy="12" r="9.5" />
      <text
        x="12"
        y={two ? 15.6 : 16.2}
        textAnchor="middle"
        fontSize={two ? 10 : 12}
        letterSpacing={two ? -0.6 : undefined}
        fontWeight={700}
        fill="currentColor"
        stroke="none"
        style={{ fontFamily: 'var(--font-ui)' }}
      >
        {n}
      </text>
    </svg>
  )
}

/** `index` is the group's 0-based position (the number shows index + 1).
 *  `bare`: a group with no icon shows the bare numeral (the bar, the dialog
 *  list); otherwise the outlined dot. */
export function GroupGlyph({
  icon,
  index,
  size = K2_ICON_PX,
  bare = false,
}: {
  icon: string | null | undefined
  index: number
  size?: number
  bare?: boolean
}): React.JSX.Element {
  const def = groupIcon(icon)
  if (def) return <GroupIconSvg def={def} size={size} />
  return bare ? <GroupNumeral n={index + 1} size={size} /> : <GroupDot n={index + 1} size={size} />
}
