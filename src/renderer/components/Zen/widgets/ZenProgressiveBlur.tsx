// K2 Companion's progressive blur (`.zen-header-blur` / `.zen-footer-blur`),
// for Zen themes that draw Companion's materials (Glass): four masked
// backdrop-blur layers, light to heavy (2/6/12/24px), and a dark tint of the
// window's canvas. The header's is heavy at the top and runs down; the
// footer's is heavy at the bottom and runs up. Never takes clicks. With
// reduced transparency there are no blur layers, only a near-opaque tint.
// The CSS is `ZEN_MATERIALS_CSS` (`lib/zen/zen-materials.ts`).

import type { CSSProperties } from 'react'
import { ZEN_PROGRESSIVE_BLUR_LAYERS, useZenMaterials } from '@/lib/zen/zen-materials'

export function ZenProgressiveBlur({ edge, style }: { edge: 'header' | 'footer'; style?: CSSProperties }): React.JSX.Element {
  const { reducedTransparency } = useZenMaterials()
  return (
    <div aria-hidden="true" className={`zen-progressive-blur zen-${edge}-blur`} data-zen-progressive-blur={edge} style={style}>
      {!reducedTransparency && ZEN_PROGRESSIVE_BLUR_LAYERS.map((_, i) => <span key={i} className={`zen-blur-layer-${i + 1}`} />)}
      <span className="zen-blur-tint" />
    </div>
  )
}
