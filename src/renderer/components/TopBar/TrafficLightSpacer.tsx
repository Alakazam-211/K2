/**
 * Reserves space for macOS window controls on desktop.
 * Hosted web returns null so the top bar content starts at the left edge.
 */
import type { JSX } from 'react'
import { TRAFFIC_LIGHT_SPACER_PX } from '@/web/features'

export default function TrafficLightSpacer(): JSX.Element | null {
  if (TRAFFIC_LIGHT_SPACER_PX <= 0) return null
  return <div style={{ width: TRAFFIC_LIGHT_SPACER_PX }} aria-hidden />
}
