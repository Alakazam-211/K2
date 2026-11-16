// The scene panel's icons: the app's line set (Lucide, already cleared for
// K2) at K2's stroke, in currentColor, hidden from assistive tech. One per
// object kind as the mock draws them (a box for a mesh, a camera, a light,
// axes for an empty), collections (a panel for a collection, layers for the
// scene, a dashed box for "Not in a collection"), and the marks on a row:
// held, driven, turns by quaternion, linked, shown or hidden. The picture's
// views (Scene, a camera, a framed object, the turntable), a picture, done.

import type { LucideIcon } from 'lucide-react'
import { Axis3dIcon, BoxIcon, CheckIcon, EyeIcon, EyeOffIcon, LayersIcon, LightbulbIcon, LinkIcon, LockIcon, PanelTopIcon, RotateCwIcon, Rotate3dIcon, ScanIcon, SquareDashedIcon, ImageIcon, TriangleAlertIcon, VideoIcon, WavesIcon } from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'

function glyph(Icon: LucideIcon, stroke = K2_ICON_STROKE): (p: { size?: number; className?: string }) => React.JSX.Element {
  return function Glyph({ size = 14, className }) {
    return <Icon size={size} strokeWidth={stroke} aria-hidden="true" focusable="false" className={className} />
  }
}

const KIND: Record<string, ReturnType<typeof glyph>> = {
  mesh: glyph(BoxIcon),
  camera: glyph(VideoIcon),
  light: glyph(LightbulbIcon),
  empty: glyph(Axis3dIcon),
}
const COLL: Record<string, ReturnType<typeof glyph>> = { scene: glyph(LayersIcon), none: glyph(SquareDashedIcon), collection: glyph(PanelTopIcon) }

/** An object's kind, as its outliner and Properties icon. */
export function KindGlyph({ kind, size = 14 }: { kind: string; size?: number }): React.JSX.Element {
  const G = KIND[kind] ?? KIND.empty
  return <G size={size} />
}
export function CollGlyph({ kind, size = 14 }: { kind: string; size?: number }): React.JSX.Element {
  const G = COLL[kind] ?? COLL.collection
  return <G size={size} />
}

export const LockGlyph = glyph(LockIcon, 2)
export const EyeGlyph = glyph(EyeIcon)
export const EyeOffGlyph = glyph(EyeOffIcon)
export const DrivenGlyph = glyph(WavesIcon, 2)
export const QuatGlyph = glyph(RotateCwIcon, 2)
export const LinkedGlyph = glyph(LinkIcon, 2)
export const FramedGlyph = glyph(ScanIcon, 2)
export const RefusedGlyph = glyph(TriangleAlertIcon, 2)
export const CameraGlyph = KIND.camera
export const SceneViewGlyph = KIND.mesh
export const TurntableGlyph = glyph(Rotate3dIcon)
export const PictureGlyph = glyph(ImageIcon)
export const DoneGlyph = glyph(CheckIcon, 2)
