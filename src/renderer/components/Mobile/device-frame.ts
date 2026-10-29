// The Mobile tab's phone frame, as numbers (research-mobile-pane-polish-v1
// §4 item 1). Pure, so the geometry is tested without a layout engine.
//
// The frame is drawn in our own CSS (DeviceFrame.tsx), never from device
// models: Apple's 3D phones have no redistribution licence.
//
// Shape numbers: the bezel and corner-radius fractions, and the fit-by-
// height-then-clamp-to-width rule (`fitDeviceFrame`), come from T3 Code
// (MIT, © 2026 T3 Tools Inc.; `packages/client-runtime/src/device/
// shapeProfile.ts` and `apps/web/src/components/device/deviceFrameLayout.ts`).
// See NOTICE.md. The cut-outs and side-button positions are K2's own.
//
// Units: every fraction is of the screen's SHORT side, so a frame keeps its
// proportions at any size (the old frame was a fixed 10 px border).

import type { SimLaneId } from './simApi'

export type DeviceShapeId = 'ios-phone' | 'ios-tablet' | 'android-phone' | 'android-tablet'

/** A side button, in portrait: the edge, where it starts and how long it is
 *  (fractions of that edge's length, from the top or the left). */
export interface FrameButton {
  edge: 'left' | 'right' | 'top'
  start: number
  length: number
}

export interface DeviceShape {
  id: DeviceShapeId
  /** Black glass between the body's edge and the screen. */
  bezel: number
  bodyRadius: number
  screenRadius: number
  /** What sits in the screen's top: an iPhone's island, a punch-hole camera. */
  cutout: 'island' | 'punch-hole' | 'none'
  buttons: readonly FrameButton[]
}

export const IOS_PHONE_SHAPE: DeviceShape = {
  id: 'ios-phone',
  bezel: 0.055,
  bodyRadius: 0.15,
  screenRadius: 0.105,
  cutout: 'island',
  buttons: [
    { edge: 'left', start: 0.17, length: 0.035 },
    { edge: 'left', start: 0.235, length: 0.065 },
    { edge: 'left', start: 0.32, length: 0.065 },
    { edge: 'right', start: 0.28, length: 0.1 },
  ],
}

export const IOS_TABLET_SHAPE: DeviceShape = {
  id: 'ios-tablet',
  bezel: 0.065,
  bodyRadius: 0.105,
  screenRadius: 0.045,
  cutout: 'none',
  buttons: [
    { edge: 'top', start: 0.8, length: 0.08 },
    { edge: 'right', start: 0.12, length: 0.07 },
    { edge: 'right', start: 0.2, length: 0.07 },
  ],
}

export const ANDROID_PHONE_SHAPE: DeviceShape = {
  id: 'android-phone',
  bezel: 0.035,
  bodyRadius: 0.115,
  screenRadius: 0.08,
  cutout: 'punch-hole',
  buttons: [
    { edge: 'right', start: 0.22, length: 0.07 },
    { edge: 'right', start: 0.33, length: 0.12 },
  ],
}

export const ANDROID_TABLET_SHAPE: DeviceShape = { ...IOS_TABLET_SHAPE, id: 'android-tablet' }

/** The shape for a device: a tablet by name, else by how square it is. */
export function resolveDeviceShape(lane: SimLaneId | null, name: string, screen: { widthPt: number; heightPt: number } | null): DeviceShape {
  const namedTablet = /\b(ipad|tablet)\b/i.test(name)
  const namedPhone = /\b(iphone|phone|pixel)\b/i.test(name)
  const short = screen ? Math.min(screen.widthPt, screen.heightPt) : 0
  const long = screen ? Math.max(screen.widthPt, screen.heightPt) : 0
  const squareish = long > 0 && short / long >= 0.6
  const tablet = namedTablet || (!namedPhone && squareish)
  if (lane === 'android') return tablet ? ANDROID_TABLET_SHAPE : ANDROID_PHONE_SHAPE
  return tablet ? IOS_TABLET_SHAPE : IOS_PHONE_SHAPE
}

/** Fit a box of `aspect` (w/h) by height, then clamp to the width left
 *  after `rightInset` (T3 Code, MIT). Never distorts the aspect. */
export function fitDeviceFrame(aspect: number, width: number, height: number, rightInset: number): { width: number; height: number } {
  const availableWidth = Math.max(0, width - rightInset)
  if (availableWidth === 0 || height <= 0 || !(aspect > 0)) return { width: 0, height: 0 }
  const byHeight = { width: height * aspect, height }
  return byHeight.width <= availableWidth ? byHeight : { width: availableWidth, height: availableWidth / aspect }
}

export interface FrameLayout {
  landscape: boolean
  /** The whole body, px. */
  width: number
  height: number
  /** The metal band, then the glass bezel inside it, px. */
  edge: number
  bezel: number
  bodyRadius: number
  screenRadius: number
  /** px per screen-short-side unit (cut-outs and buttons scale with it). */
  unit: number
  /** Side buttons for this orientation, px, relative to the body. */
  buttons: Array<{ edge: 'left' | 'right' | 'top' | 'bottom'; start: number; length: number; thickness: number }>
}

/** Where a portrait edge goes once the phone turns a quarter left (the
 *  top of the phone to the left): right → top, left → bottom, top → left. */
const LANDSCAPE_EDGE = { right: 'top', left: 'bottom', top: 'left' } as const

/**
 * The frame for a screen of `screen` points inside a `stage` px box.
 * `screenOnly` drops the body: just the screen with its rounded corners.
 */
export function frameLayout(
  shape: DeviceShape,
  screen: { widthPt: number; heightPt: number },
  stage: { width: number; height: number },
  opts: { rightInset?: number; screenOnly?: boolean } = {},
): FrameLayout {
  const landscape = screen.widthPt > screen.heightPt
  const short = Math.min(screen.widthPt, screen.heightPt)
  const long = Math.max(screen.widthPt, screen.heightPt)
  const ratio = short > 0 ? long / short : 19.5 / 9
  const b = opts.screenOnly ? 0 : shape.bezel
  const shortSide = 1 + 2 * b
  const longSide = ratio + 2 * b
  const aspect = landscape ? longSide / shortSide : shortSide / longSide
  const fit = fitDeviceFrame(aspect, stage.width, stage.height, opts.rightInset ?? 0)
  const bodyShort = Math.min(fit.width, fit.height)
  const unit = bodyShort / shortSide
  const bezel = b * unit
  const edge = opts.screenOnly ? 0 : Math.min(bezel / 2, Math.max(1, 0.012 * unit))
  const thickness = Math.max(2, 0.014 * unit)
  const bodyLong = Math.max(fit.width, fit.height)
  const buttons = opts.screenOnly
    ? []
    : shape.buttons.map((btn) => {
        const edgeName = landscape ? LANDSCAPE_EDGE[btn.edge] : btn.edge
        // Top / bottom of a portrait body run along its short side.
        const along = edgeName === 'top' || edgeName === 'bottom' ? (landscape ? bodyLong : bodyShort) : landscape ? bodyShort : bodyLong
        return { edge: edgeName, start: btn.start * along, length: btn.length * along, thickness }
      })
  return {
    landscape,
    width: fit.width,
    height: fit.height,
    edge,
    bezel,
    bodyRadius: (opts.screenOnly ? shape.screenRadius : shape.bodyRadius) * unit,
    screenRadius: shape.screenRadius * unit,
    unit,
    buttons,
  }
}

/** The cut-out over the screen, px relative to the screen box (null: none). */
export function cutoutBox(
  shape: DeviceShape,
  layout: Pick<FrameLayout, 'unit' | 'landscape'>,
  screenBox: { width: number; height: number },
): { left: number; top: number; width: number; height: number } | null {
  const u = layout.unit
  if (shape.cutout === 'none' || u <= 0) return null
  const [w, h, inset] = shape.cutout === 'island' ? [0.31 * u, 0.09 * u, 0.03 * u] : [0.06 * u, 0.06 * u, 0.035 * u]
  // Landscape: the top of the phone is on the left.
  if (layout.landscape) return { left: inset, top: (screenBox.height - w) / 2, width: h, height: w }
  return { left: (screenBox.width - w) / 2, top: inset, width: w, height: h }
}
