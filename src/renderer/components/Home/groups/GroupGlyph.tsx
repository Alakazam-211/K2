// One Home group's mark: its Lucide icon, drawn squared like K2's own icons
// (style A3, `components/icons/K2Icons.tsx`: 1.75 stroke, square caps,
// mitred joins, currentColor; 16 px, or GROUP_BAR_ICON_PX bare in the
// group bar), or a numbered dot when it has none or its name isn't in this
// build's list. The dot's number is the group's position (its ⌥⌘N).

import { groupIcon, type GroupIconDef } from '@/lib/group-icons'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'

/** The group bar draws its icons bare at half the mock's 16 px. */
export const GROUP_BAR_ICON_PX = 8

/** A curated icon in the A3 look. */
export function GroupIconSvg({ def, size = K2_ICON_PX }: { def: GroupIconDef; size?: number }): React.JSX.Element {
  const Icon = def.Icon
  return (
    <Icon
      size={size}
      strokeWidth={K2_ICON_STROKE}
      strokeLinecap="square"
      strokeLinejoin="miter"
      color="currentColor"
      aria-hidden="true"
      focusable="false"
      data-group-icon={def.name}
    />
  )
}

/** The numbered dot for a group with no icon (1-based position). */
export function GroupDot({ n, size = K2_ICON_PX }: { n: number; size?: number }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      data-group-dot={n}
      className="inline-flex items-center justify-center rounded-full border border-current font-medium leading-none tabular-nums"
      style={{ minWidth: size, height: size, fontSize: Math.max(5, Math.round(size * 0.56)), padding: size >= 12 ? '0 3px' : 0 }}
    >
      {n}
    </span>
  )
}

/** `index` is the group's 0-based position (the dot shows index + 1). */
export function GroupGlyph({ icon, index, size }: { icon: string | null | undefined; index: number; size?: number }): React.JSX.Element {
  const def = groupIcon(icon)
  return def ? <GroupIconSvg def={def} size={size} /> : <GroupDot n={index + 1} size={size} />
}
