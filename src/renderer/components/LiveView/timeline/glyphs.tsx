// The lanes' icons: the app's line set at K2's stroke, in currentColor,
// hidden from assistive tech (whatever carries one names itself in words).
// Flags and markers on the ruler are small drawn shapes, also currentColor.

import type { LucideIcon } from 'lucide-react'
import {
  AudioLinesIcon,
  BanIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronsLeftRightIcon,
  EyeOffIcon,
  CloudOffIcon,
  FilmIcon,
  ImageIcon,
  ImageOffIcon,
  InfoIcon,
  KeyboardIcon,
  LayersIcon,
  LockIcon,
  PauseIcon,
  PlayIcon,
  RepeatIcon,
  SkipBackIcon,
  SkipForwardIcon,
  StepBackIcon,
  StepForwardIcon,
  TypeIcon,
  VolumeXIcon,
  ZoomInIcon,
  ZoomOutIcon,
} from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import type { KindGlyph } from '@/lib/live-view/timeline/labels'

function glyph(Icon: LucideIcon): (p: { size?: number; className?: string }) => React.JSX.Element {
  return function Glyph({ size = 14, className }) {
    return <Icon size={size} strokeWidth={K2_ICON_STROKE} aria-hidden="true" focusable="false" className={className} />
  }
}

export const MutedGlyph = glyph(VolumeXIcon)
export const OffGlyph = glyph(EyeOffIcon)
export const LockGlyph = glyph(LockIcon)
export const StopGlyph = glyph(BanIcon)
export const ZoomInGlyph = glyph(ZoomInIcon)
export const ZoomOutGlyph = glyph(ZoomOutIcon)
export const FitGlyph = glyph(ChevronsLeftRightIcon)
export const DownGlyph = glyph(ChevronDownIcon)
export const CheckGlyph = glyph(CheckIcon)
// The monitor and transport (T4).
export const PlayGlyph = glyph(PlayIcon)
export const PauseGlyph = glyph(PauseIcon)
export const FrameBackGlyph = glyph(StepBackIcon)
export const FrameFwdGlyph = glyph(StepForwardIcon)
export const PrevEditGlyph = glyph(SkipBackIcon)
export const NextEditGlyph = glyph(SkipForwardIcon)
export const LoopGlyph = glyph(RepeatIcon)
export const KeysGlyph = glyph(KeyboardIcon)
export const InfoGlyph = glyph(InfoIcon)
export const NoPreviewGlyph = glyph(ImageOffIcon)
export const OfflineGlyph = glyph(CloudOffIcon)
export const NestGlyph = glyph(LayersIcon)
export const ImageGlyph = glyph(ImageIcon)
export const AudioGlyph = glyph(AudioLinesIcon)
export const FilmCardGlyph = glyph(FilmIcon)

const KIND: Record<KindGlyph, (p: { size?: number }) => React.JSX.Element> = {
  title: glyph(TypeIcon),
  nest: glyph(LayersIcon),
  video: glyph(FilmIcon),
  audio: glyph(AudioLinesIcon),
}
export function KindIcon({ kind, size = 10 }: { kind: KindGlyph; size?: number }): React.JSX.Element {
  const G = KIND[kind]
  return <G size={size} />
}

/** A marker's flag on the ruler (a pentagon). */
export function FlagShape(): React.JSX.Element {
  return (
    <svg viewBox="0 0 8 12" aria-hidden="true" focusable="false">
      <path d="M0 0H8V8L4 12L0 8Z" fill="currentColor" />
    </svg>
  )
}

/** The monitor's playhead head at the foot of the ruler. */
export function HeadShape(): React.JSX.Element {
  return (
    <svg viewBox="0 0 12 9" aria-hidden="true" focusable="false">
      <path d="M0 0H12L6 9Z" fill="currentColor" />
    </svg>
  )
}

/** A marker on a clip (a small triangle) or on a track (a diamond). */
export function ClipMarkShape(): React.JSX.Element {
  return (
    <svg viewBox="0 0 8 6" aria-hidden="true" focusable="false">
      <path d="M4 0L8 6H0Z" fill="currentColor" />
    </svg>
  )
}
export function TrackMarkShape(): React.JSX.Element {
  return (
    <svg viewBox="0 0 8 8" aria-hidden="true" focusable="false">
      <path d="M4 0L8 4L4 8L0 4Z" fill="currentColor" />
    </svg>
  )
}

/** A transition's cross over the cut. */
export function TransitionShape(): React.JSX.Element {
  return (
    <svg viewBox="0 0 100 20" preserveAspectRatio="none" aria-hidden="true" focusable="false">
      <path d="M0 0L100 20V0L0 20Z" fill="currentColor" />
    </svg>
  )
}
