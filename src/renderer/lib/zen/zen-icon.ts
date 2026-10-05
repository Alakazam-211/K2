// Which icon the Zen toggle draws (Rosson 2026-10-04, picking a new one).
//
// TEMPORARY comparison: while `ZEN_ICON_PREVIEW` is true, the app top bar
// AND Zen's top band each show all three candidate toggles side by side
// (ripples, enso, bonsai, in that order) in place of the single toggle.
// Every one of them is a real Zen toggle with the same handler, and its
// tooltip names it ("Option 1: Stone & ripples", ...).
//
// To finalize: set `ZEN_ICON_CHOICE` to the pick and `ZEN_ICON_PREVIEW` to
// false. The single toggle then draws the chosen icon. The icons themselves
// are `components/Zen/ZenIcon.tsx`.

export type ZenIconVariant = 'ripples' | 'enso' | 'bonsai'

/** TEMPORARY: show all three candidates side by side. */
export const ZEN_ICON_PREVIEW: boolean = true

/** The icon the single toggle draws when the preview is off. */
export const ZEN_ICON_CHOICE: ZenIconVariant = 'ripples'

export interface ZenIconOption {
  readonly variant: ZenIconVariant
  /** The preview tooltip's name for it. */
  readonly label: string
}

/** The candidates, in the order the preview shows them. */
export const ZEN_ICON_OPTIONS: readonly ZenIconOption[] = [
  { variant: 'ripples', label: 'Option 1: Stone & ripples' },
  { variant: 'enso', label: 'Option 2: Ensō' },
  { variant: 'bonsai', label: 'Option 3: Bonsai' },
]

/** The toggles to draw: all three while previewing, else the chosen one. */
export function zenToggleIcons(
  preview: boolean = ZEN_ICON_PREVIEW,
  choice: ZenIconVariant = ZEN_ICON_CHOICE,
): readonly ZenIconOption[] {
  if (preview === true) return ZEN_ICON_OPTIONS
  const chosen = ZEN_ICON_OPTIONS.find((o) => o.variant === choice)
  if (!chosen) throw new Error(`zen icon: unknown choice "${String(choice)}"`)
  return [chosen]
}
