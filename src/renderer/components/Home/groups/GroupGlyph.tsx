// One Home group's mark: its Lucide icon, drawn squared like K2's own icons
// (style A3, `components/icons/K2Icons.tsx`: 16 px, 1.75 stroke, square
// caps, mitred joins, currentColor), or a numbered dot when it has none or
// its name isn't in this build's list.

import { groupIcon, type GroupIconDef } from '@/lib/group-icons'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'

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
export function GroupDot({ n }: { n: number }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      data-group-dot={n}
      className="inline-flex h-4 min-w-4 items-center justify-center rounded-full border border-current px-[3px] text-[9px] font-medium leading-none tabular-nums"
    >
      {n}
    </span>
  )
}

/** `index` is the group's 0-based position (the dot shows index + 1). */
export function GroupGlyph({ icon, index }: { icon: string | null | undefined; index: number }): React.JSX.Element {
  const def = groupIcon(icon)
  return def ? <GroupIconSvg def={def} /> : <GroupDot n={index + 1} />
}
