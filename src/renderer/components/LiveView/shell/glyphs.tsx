// The shell's icons: the app's line set at K2's stroke, in currentColor,
// hidden from assistive tech (every control names itself in words).

import type { LucideIcon } from 'lucide-react'
import {
  ArrowDownIcon,
  ArrowLeftIcon,
  ArrowRightIcon,
  ArrowUpIcon,
  AtSignIcon,
  BoxIcon,
  ChevronDownIcon,
  CircleCheckIcon,
  CopyIcon,
  CrosshairIcon,
  FilmIcon,
  HistoryIcon,
  InfoIcon,
  PanelRightIcon,
  RefreshCwIcon,
  SendIcon,
  TriangleAlertIcon,
  UserRoundIcon,
  WifiOffIcon,
  XIcon,
} from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'

function glyph(Icon: LucideIcon): (p: { size?: number; className?: string }) => React.JSX.Element {
  return function Glyph({ size = 14, className }) {
    return <Icon size={size} strokeWidth={K2_ICON_STROKE} aria-hidden="true" focusable="false" className={className} />
  }
}

export const FilmGlyph = glyph(FilmIcon)
export const CubeGlyph = glyph(BoxIcon)
export const PanelGlyph = glyph(PanelRightIcon)
export const FollowGlyph = glyph(CrosshairIcon)
export const SendGlyph = glyph(SendIcon)
export const AtGlyph = glyph(AtSignIcon)
export const CloseGlyph = glyph(XIcon)
export const WarnGlyph = glyph(TriangleAlertIcon)
export const OfflineGlyph = glyph(WifiOffIcon)
export const RetryGlyph = glyph(RefreshCwIcon)
export const CopyGlyph = glyph(CopyIcon)
export const HistoryGlyph = glyph(HistoryIcon)
export const OkGlyph = glyph(CircleCheckIcon)
export const InfoGlyph = glyph(InfoIcon)
export const YouGlyph = glyph(UserRoundIcon)
export const ChevronGlyph = glyph(ChevronDownIcon)

const ARROWS = { left: glyph(ArrowLeftIcon), right: glyph(ArrowRightIcon), up: glyph(ArrowUpIcon), down: glyph(ArrowDownIcon) }
export function EdgeArrow({ side, size = 12 }: { side: 'left' | 'right' | 'top' | 'bottom'; size?: number }): React.JSX.Element {
  const A = ARROWS[side === 'top' ? 'up' : side === 'bottom' ? 'down' : side]
  return <A size={size} />
}
