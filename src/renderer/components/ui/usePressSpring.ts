// Press spring (from K2 Companion's usePressSpring): while a pointer is down a
// control grows to the style's --motion-press-scale on a stiff spring; on
// release it springs back on a looser one, overshooting a touch and settling.
// Only the `ui` primitives use it, and only a style with a press scale above 1
// (Glass) ever moves; at 1 (Terminal, and everything inside a Garden through
// the Zen shield) the press does nothing at all. The element's individual
// `scale` is written, so an existing transform on it is never disturbed.

import { useCallback, useEffect, useRef } from 'react'

const DOWN = { k: 550, c: 30 }
const UP = { k: 300, c: 16 }

type Spring = { value: number; velocity: number; target: number; k: number; c: number }

/** One integration step (unit mass); false once settled on the target. */
export function stepSpring(s: Spring, dt: number): boolean {
  const force = -s.k * (s.value - s.target) - s.c * s.velocity
  s.velocity += force * dt
  s.value += s.velocity * dt
  if (Math.abs(s.value - s.target) < 0.0005 && Math.abs(s.velocity) < 0.005) {
    s.value = s.target
    s.velocity = 0
    return false
  }
  return true
}

function reducedMotion(): boolean {
  try {
    return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
  } catch {
    return false
  }
}

function pressScale(el: HTMLElement): number {
  const v = parseFloat(getComputedStyle(el).getPropertyValue('--motion-press-scale'))
  return Number.isFinite(v) && v > 1 ? v : 1
}

export type PressSpring = {
  press: () => void
  release: () => void
  /** Stop, and leave the element as it was. */
  detach: () => void
}

/** The press spring on `el`, driven by the caller. */
export function attachPressSpring(el: HTMLElement): PressSpring {
  const s: Spring = { value: 1, velocity: 0, target: 1, ...DOWN }
  let frame = 0
  let last = 0
  let down = false
  const paint = (): void => {
    if (s.value === 1) el.style.removeProperty('scale')
    else el.style.setProperty('scale', s.value.toFixed(4))
  }
  const tick = (t: number): void => {
    const dt = Math.min(0.032, last ? (t - last) / 1000 : 1 / 60)
    last = t
    const moving = stepSpring(s, dt)
    paint()
    if (moving) {
      frame = requestAnimationFrame(tick)
    } else {
      frame = 0
      last = 0
    }
  }
  const wake = (): void => {
    if (!frame) frame = requestAnimationFrame(tick)
  }
  return {
    press: () => {
      if (reducedMotion()) return
      const scale = pressScale(el)
      if (scale <= 1) return
      down = true
      Object.assign(s, { target: scale }, DOWN)
      wake()
    },
    release: () => {
      if (!down) return
      down = false
      Object.assign(s, { target: 1 }, UP)
      wake()
    },
    detach: () => {
      if (frame) cancelAnimationFrame(frame)
      frame = 0
      el.style.removeProperty('scale')
    },
  }
}

/** A ref callback that gives the element the press spring. It listens on its
 *  own passive pointer events, so the control's handlers are untouched. */
export function usePressSpring<T extends HTMLElement>(): (el: T | null) => void {
  const detach = useRef<(() => void) | null>(null)
  useEffect(() => () => detach.current?.(), [])
  return useCallback((el: T | null) => {
    detach.current?.()
    detach.current = null
    if (!el) return
    const spring = attachPressSpring(el)
    const onDown = (): void => {
      if ((el as unknown as HTMLButtonElement).disabled) return
      spring.press()
    }
    const onUp = (): void => spring.release()
    el.addEventListener('pointerdown', onDown, { passive: true })
    el.addEventListener('pointerup', onUp, { passive: true })
    el.addEventListener('pointercancel', onUp, { passive: true })
    el.addEventListener('pointerleave', onUp, { passive: true })
    detach.current = () => {
      el.removeEventListener('pointerdown', onDown)
      el.removeEventListener('pointerup', onUp)
      el.removeEventListener('pointercancel', onUp)
      el.removeEventListener('pointerleave', onUp)
      spring.detach()
    }
  }, [])
}
