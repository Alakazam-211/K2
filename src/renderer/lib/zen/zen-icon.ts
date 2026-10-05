// The Zen icon: the top bar's way into Zen (`ZenTopBarToggle`) draws it.
// Inside Zen, the top band's toggle stays the "Zen" label with its switch.
//
// Rosson 2026-10-04 compared three candidates side by side (stone & ripples,
// ensō, bonsai) and picked the ensō. All three stay drawable in
// `components/Zen/ZenIcon.tsx`, so a change of mind is this one constant.

export type ZenIconVariant = 'ripples' | 'enso' | 'bonsai'

/** The icon the top bar's Zen toggle draws. */
export const ZEN_ICON_CHOICE: ZenIconVariant = 'enso'
