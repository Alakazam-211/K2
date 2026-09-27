import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { TRAFFIC_LIGHT_Y_NUDGE_PX } from './traffic-lights'
import { createWindowCornerController, windowCornerRadius } from './window-corners'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')

function cfgBody(rust: string, marker: string): string {
  const start = rust.indexOf(marker)
  expect(start).toBeGreaterThan(-1)
  const open = rust.indexOf('{', start)
  expect(open).toBeGreaterThan(start)
  let depth = 0
  let end = -1
  for (let i = open; i < rust.length; i++) {
    if (rust[i] === '{') depth++
    else if (rust[i] === '}') {
      depth--
      if (depth === 0) {
        end = i + 1
        break
      }
    }
  }
  expect(end).toBeGreaterThan(open)
  return rust.slice(open, end)
}

describe('window corner radius', () => {
  const squareCases = [
    { scheme: 'dark', palette: 'charcoal', gaps: null, density: 'compact', inset: 0 },
    { scheme: 'light', palette: 'paper', gaps: '', density: 'compact', inset: 0 },
    { scheme: 'dark', palette: 'charcoal', gaps: 'regular', density: 'regular', inset: 6 },
    { scheme: 'light', palette: 'paper', gaps: 'regular', density: 'regular', inset: 6 },
    { scheme: 'dark', palette: 'paper', gaps: 'spacious', density: 'spacious', inset: 12 },
    { scheme: 'light', palette: 'charcoal', gaps: 'spacious', density: 'spacious', inset: 12 },
  ] as const

  it('returns 0.5 for square in both schemes and every density, else 0', () => {
    for (const row of squareCases) {
      expect(windowCornerRadius({ styleId: 'square', ...row })).toBe(0.5)
    }

    for (const styleId of ['glass', 'bezel', 'unknown', 'classic', '', null, undefined]) {
      for (const row of squareCases) {
        expect(windowCornerRadius({ styleId, ...row })).toBe(0)
      }
    }
  })
})

describe('window corner re-apply', () => {
  it('schedules Square compact (inset 0) on resize and fullscreen enter/exit', () => {
    const applied: number[] = []
    const scheduled: Array<() => void> = []
    let styleId: string | null = 'square'
    let inset = 0
    const corners = createWindowCornerController({
      isMac: () => true,
      read: () => ({
        styleId,
        scheme: 'dark',
        palette: 'charcoal',
        gaps: null,
        density: 'compact',
        inset,
      }),
      apply: (radius) => {
        applied.push(radius)
      },
      schedule: (fn) => {
        scheduled.push(fn)
      },
    })

    corners.onResize()
    expect(scheduled).toHaveLength(1)
    expect(applied).toHaveLength(0)
    scheduled[0]()
    expect(applied).toEqual([0.5])

    corners.onFullscreen()
    expect(scheduled).toHaveLength(2)
    scheduled[1]()
    expect(applied[1]).toBe(0.5)

    corners.onFullscreen()
    expect(scheduled).toHaveLength(3)
    scheduled[2]()
    expect(applied[2]).toBe(0.5)

    const before = scheduled.length
    corners.onResize()
    corners.onFullscreen()
    expect(scheduled).toHaveLength(before + 1)

    styleId = 'glass'
    inset = 10
    scheduled.at(-1)!()
    expect(applied.at(-1)).toBe(0)

    const linux: number[] = []
    const linuxScheduled: Array<() => void> = []
    const off = createWindowCornerController({
      isMac: () => false,
      read: () => ({ styleId: 'square', inset: 0 }),
      apply: (radius) => {
        linux.push(radius)
      },
      schedule: (fn) => {
        linuxScheduled.push(fn)
      },
    })
    off.onResize()
    off.onFullscreen()
    off.reapply()
    expect(linuxScheduled).toHaveLength(0)
    expect(linux).toHaveLength(0)
  })
})

describe('window corner source locks', () => {
  it('keeps the non-mac command free of the private setter, and does not branch WindowControls', () => {
    const rust = readFileSync(resolve(root, 'src-tauri/src/commands/window_corners.rs'), 'utf8')
    const body = cfgBody(rust, '#[cfg(not(target_os = "macos"))]')
    expect(body).not.toMatch(/cornerRadius|_setCornerRadius:/)
    expect(rust).not.toMatch(/["']_cornerRadius["']/)
    expect(rust).not.toContain('DWMWA_WINDOW_CORNER_PREFERENCE')
    expect(rust).not.toContain('EffectsBuilder')
    expect(rust).not.toContain('keyWindow')
    expect(rust).toContain('_setCornerRadius:')
    expect(rust).toContain('invalidateShadow')
    const setter = rust.slice(rust.indexOf('unsafe fn set_radius'))
    const setAt = setter.indexOf('msg_send![ns_window, _setCornerRadius:')
    const shadowAt = setter.indexOf('msg_send![ns_window, invalidateShadow]')
    expect(setAt).toBeGreaterThan(-1)
    expect(shadowAt).toBeGreaterThan(setAt)

    const controls = readFileSync(
      resolve(root, 'src/renderer/components/TopBar/WindowControls.tsx'),
      'utf8',
    )
    expect(controls.includes('data-style')).toBe(false)

    const lights = readFileSync(resolve(root, 'src-tauri/src/commands/traffic_lights.rs'), 'utf8')
    const sig = lights.slice(
      lights.indexOf('pub fn set_traffic_light_inset'),
      lights.indexOf('{', lights.indexOf('pub fn set_traffic_light_inset')),
    )
    expect(sig).not.toMatch(/radius|corner/i)
  })

  it('does not read the traffic-light nudge, which stays 3', () => {
    expect(TRAFFIC_LIGHT_Y_NUDGE_PX).toBe(3)
    const decision = readFileSync(resolve(root, 'src/renderer/lib/window-corners.ts'), 'utf8')
    const lights = readFileSync(resolve(root, 'src/renderer/lib/traffic-lights.ts'), 'utf8')
    expect(decision).not.toContain('TRAFFIC_LIGHT_Y_NUDGE_PX')
    expect(decision).not.toContain('--inset-window')
    expect(decision).not.toMatch(/inset\s*<=\s*0/)
    expect(lights).toMatch(/export const TRAFFIC_LIGHT_Y_NUDGE_PX = 3/)
  })

  it('reapplies from the stamp, resize, fullscreen, and setTitle without an inset bail', () => {
    const style = readFileSync(resolve(root, 'src/renderer/stores/style.ts'), 'utf8')
    const stamp = style.slice(
      style.indexOf('export function stampStyleAttributes'),
      style.indexOf('const trafficLights'),
    )
    expect(stamp).toContain('syncWindowCorners()')
    const write = style.slice(style.indexOf('function writeMirror'), style.indexOf('export function readMirror'))
    expect(write).not.toContain('WindowCorner')
    expect(write).not.toContain('set_window_corner_radius')

    const listeners = style.slice(
      style.indexOf("addEventListener('resize'"),
      style.indexOf('function writeMirror'),
    )
    expect(listeners).toContain('windowCorners.onResize()')
    expect(listeners.match(/windowCorners\.onFullscreen\(\)/g)).toHaveLength(2)
    expect(listeners).not.toMatch(/lastTrafficInset\s*<=\s*0/)
    const resizeBlock = listeners.slice(0, listeners.indexOf("addEventListener('fullscreenchange'"))
    expect(resizeBlock).toContain('windowCorners.onResize()')
    expect(resizeBlock).not.toMatch(/return/)
    expect(resizeBlock).not.toMatch(/inset|lastTrafficInset|--inset-window/)

    const app = readFileSync(resolve(root, 'src/renderer/App.tsx'), 'utf8')
    const title = app.slice(app.indexOf('setTitle(title)'), app.indexOf('StyledKesselProvider'))
    expect(title).toContain('reapplyWindowCorners()')
    expect(title).not.toMatch(/inset/)

    const section = readFileSync(
      resolve(root, 'src/renderer/components/Settings/sections/StylesSection.tsx'),
      'utf8',
    )
    const preview = section.slice(section.indexOf('const preview'), section.indexOf('const baseSel'))
    expect(preview).toContain('stampStyleAttributes(sel)')
    expect(preview).toContain('stampStyleAttributes(committed)')
    expect(preview).not.toContain('applyStyle')
    expect(preview).not.toContain('localStorage')

    const lib = readFileSync(resolve(root, 'src-tauri/src/lib.rs'), 'utf8')
    expect(lib).toContain('commands::window_corners::set_window_corner_radius')
  })
})
