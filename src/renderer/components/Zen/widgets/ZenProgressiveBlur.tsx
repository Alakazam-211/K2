// K2 Companion's progressive blur (`.zen-header-blur` / `.zen-footer-blur`),
// for Zen themes that draw Companion's materials (Glass): four masked
// backdrop-blur layers, light to heavy (2/6/12/24px), and a dark tint of the
// window's canvas. The header's is heavy at the top and runs down; the
// footer's is heavy at the bottom and runs up. Never takes clicks. With
// reduced transparency there are no blur layers, only a near-opaque tint.
// 0.46.3: with Light blur on (Garden settings, this computer) a stack draws
// two layers (4/16px) instead of four, like the phone's Diagnostics → Light
// blur. The CSS is `ZEN_MATERIALS_CSS` (`lib/zen/zen-materials.ts`).

import type { CSSProperties } from 'react'
import { useZenMaterials, zenBlurLayerClass, zenProgressiveBlurLayers } from '@/lib/zen/zen-materials'

export function ZenProgressiveBlur({ edge, style }: { edge: 'header' | 'footer'; style?: CSSProperties }): React.JSX.Element {
  const { reducedTransparency, lightBlur = false } = useZenMaterials()
  return (
    <div
      aria-hidden="true"
      className={`zen-progressive-blur zen-${edge}-blur`}
      data-zen-progressive-blur={edge}
      data-zen-blur-layers={reducedTransparency ? 0 : zenProgressiveBlurLayers(lightBlur).length}
      style={style}
    >
      {!reducedTransparency &&
        zenProgressiveBlurLayers(lightBlur).map((_, i) => <span key={i} className={zenBlurLayerClass(lightBlur, i)} />)}
      <span className="zen-blur-tint" />
    </div>
  )
}
