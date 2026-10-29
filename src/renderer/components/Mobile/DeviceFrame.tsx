// The phone frame around the live screen (research-mobile-pane-polish-v1
// §4 item 1): a body tinted from the theme (light on light themes, dark on
// dark), black glass, proportional corners, an iPhone island or an Android
// punch-hole, faint side buttons, a drop shadow and a soft floor shadow.
// Our own CSS; the numbers are in device-frame.ts.
//
// The frame only draws. The screen's content (canvas, input, overlays) is
// `children`, so a later redesign restyles this file without touching input.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { cutoutBox, type DeviceShape, type FrameLayout } from './device-frame'

/** The element's content box, kept current with a ResizeObserver. */
export function useElementSize<T extends HTMLElement>(): [React.RefObject<T | null>, { width: number; height: number }] {
  const ref = useRef<T | null>(null)
  const [size, setSize] = useState({ width: 0, height: 0 })
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const read = (): void => {
      const s = getComputedStyle(el)
      const padX = (parseFloat(s.paddingLeft) || 0) + (parseFloat(s.paddingRight) || 0)
      const padY = (parseFloat(s.paddingTop) || 0) + (parseFloat(s.paddingBottom) || 0)
      const width = Math.max(0, el.clientWidth - padX)
      const height = Math.max(0, el.clientHeight - padY)
      setSize((prev) => (prev.width === width && prev.height === height ? prev : { width, height }))
    }
    read()
    if (typeof ResizeObserver !== 'function') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, size]
}

/** The body's tint: the theme's background pulled toward its text colour,
 *  so the phone is light silver on a light theme and graphite on a dark one. */
const BODY = 'color-mix(in srgb, var(--color-bg) 80%, var(--color-text-primary))'
const BODY_HI = 'color-mix(in srgb, var(--color-bg) 88%, var(--color-text-primary))'
const BODY_LO = 'color-mix(in srgb, var(--color-bg) 70%, var(--color-text-primary))'
const RIM = 'color-mix(in srgb, var(--color-bg) 55%, var(--color-text-primary))'

export interface DeviceFrameProps {
  shape: DeviceShape
  layout: FrameLayout
  /** Just the screen with its corners (⋯ → Frame off). */
  screenOnly?: boolean
  /** Dim the screen (a reconnect keeps the last frame, dimmed). */
  dimmed?: boolean
  children: React.ReactNode
  className?: string
  style?: React.CSSProperties
}

/** Rotation turns the phone over this long (research §4 item 7). */
export const ROTATE_MS = 320

export function DeviceFrame({ shape, layout, screenOnly = false, dimmed = false, children, className, style }: DeviceFrameProps): React.JSX.Element {
  const { width, height, edge, bezel, bodyRadius, screenRadius } = layout
  // Rotation: the new frame starts in the old pose and turns into place,
  // instead of the aspect snapping. Skipped for reduced motion.
  const rootRef = useRef<HTMLDivElement | null>(null)
  const wasLandscape = useRef(layout.landscape)
  useEffect(() => {
    if (wasLandscape.current === layout.landscape) return
    wasLandscape.current = layout.landscape
    const el = rootRef.current
    if (!el || typeof el.animate !== 'function') return
    if (typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
    el.animate([{ transform: `rotate(${layout.landscape ? 90 : -90}deg)` }, { transform: 'rotate(0deg)' }], {
      duration: ROTATE_MS,
      easing: 'cubic-bezier(.2,.8,.2,1)',
    })
  }, [layout.landscape])
  const screenBox = { width: Math.max(0, width - 2 * bezel), height: Math.max(0, height - 2 * bezel) }
  const cut = screenOnly ? null : cutoutBox(shape, layout, screenBox)
  return (
    <div
      ref={rootRef}
      className={`relative flex-shrink-0 ${className ?? ''}`}
      style={{ width, height, ...style }}
      data-mobile-frame={screenOnly ? 'screen-only' : shape.id}
      data-mobile-bezel={layout.landscape ? 'landscape' : 'portrait'}
    >
      {screenOnly ? null : (
        <div
          aria-hidden
          className="pointer-events-none absolute left-1/2 -translate-x-1/2 rounded-[50%] bg-black/30 blur-md"
          style={{ bottom: -Math.max(6, height * 0.03), width: width * 0.7, height: Math.max(6, height * 0.03) }}
          data-mobile-floor=""
        />
      )}
      {layout.buttons.map((b, i) => {
        const vertical = b.edge === 'left' || b.edge === 'right'
        const pos: React.CSSProperties = vertical
          ? { top: b.start, height: b.length, width: b.thickness, [b.edge]: -b.thickness + 1 }
          : { left: b.start, width: b.length, height: b.thickness, [b.edge]: -b.thickness + 1 }
        return (
          <div
            key={i}
            aria-hidden
            className="pointer-events-none absolute"
            style={{ ...pos, background: RIM, borderRadius: b.thickness }}
            data-mobile-frame-button={b.edge}
          />
        )
      })}
      <div
        className="absolute inset-0"
        style={{
          borderRadius: bodyRadius,
          padding: edge,
          background: screenOnly ? 'transparent' : `linear-gradient(145deg, ${BODY_HI} 0%, ${BODY} 50%, ${BODY_LO} 100%)`,
          boxShadow: screenOnly
            ? '0 10px 30px -12px rgba(0,0,0,.45)'
            : `inset 0 0 0 1px ${RIM}, 0 18px 36px -12px rgba(0,0,0,.45), 0 4px 10px rgba(0,0,0,.18)`,
        }}
      >
        <div
          className="h-full w-full bg-black"
          style={{ borderRadius: Math.max(0, bodyRadius - edge), padding: Math.max(0, bezel - edge) }}
        >
          <div
            className="relative h-full w-full overflow-hidden bg-black transition-[filter] duration-300"
            style={{ borderRadius: screenRadius, filter: dimmed ? 'brightness(0.55) saturate(0.6)' : undefined }}
            data-mobile-frame-screen=""
          >
            {children}
            {cut ? (
              <div
                aria-hidden
                className="pointer-events-none absolute rounded-full bg-black"
                style={{ left: cut.left, top: cut.top, width: cut.width, height: cut.height }}
                data-mobile-cutout={shape.cutout}
              />
            ) : null}
          </div>
        </div>
      </div>
    </div>
  )
}
