/**
 * Reserves space for macOS window controls on desktop.
 * Hosted web returns null so the top bar content starts at the left edge.
 * Width is `--k2-stoplight-spacer` (globals.css), which stores/style.ts
 * resizes on every app-zoom change so it matches the unscaled buttons.
 */
import type { JSX } from 'react'
import { TRAFFIC_LIGHT_SPACER_PX } from '@/web/features'

export default function TrafficLightSpacer(): JSX.Element | null {
  if (TRAFFIC_LIGHT_SPACER_PX <= 0) return null
  return <div className="k2-stoplight-spacer" data-testid="stoplight-spacer" aria-hidden />
}
