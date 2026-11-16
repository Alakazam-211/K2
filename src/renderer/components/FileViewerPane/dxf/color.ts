// Entity colours. True colour (group 420) wins over the ACI index (62),
// which wins over BYLAYER:
//   1. a true colour (420), when present;
//   2. ACI 1–255 as it is;
//   3. ACI 0 (BYBLOCK): the enclosing INSERT's colour (the foreground at top level);
//   4. ACI 256 (BYLAYER) or none: the layer's colour, else the foreground.
// Colours from the file stay true to the file; only pure white and black
// become the theme's foreground, so they show on either scheme.

import type { IEntity } from 'dxf-parser'
import type { EntityExtras } from './parse-extras'

/** The theme foreground: ACI 7 is "white on black, black on white". */
export const FG = 'currentColor'

/** 0xRRGGBB → `#rrggbb`; pure white or black → the foreground. */
export function hex(rgb: number | undefined): string {
  if (typeof rgb !== 'number' || !Number.isFinite(rgb)) return FG
  // Pure white/black would vanish on one theme or the other.
  if (rgb === 0xffffff || rgb === 0) return FG
  return `#${rgb.toString(16).padStart(6, '0')}`
}

export function entityColor(e: IEntity & EntityExtras, blockColor: string, layerColor: string | undefined): string {
  if (typeof e.trueColor === 'number') return hex(e.trueColor)
  const idx = e.colorIndex
  if (idx === 0) return blockColor
  // dxf-parser resolves ACI 1–255 to RGB in `color`.
  if (typeof idx === 'number' && idx > 0 && idx < 256) return hex(e.color)
  return layerColor ?? FG
}
