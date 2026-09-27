import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  TRAFFIC_LIGHT_Y_NUDGE_PX,
  createTrafficLightController,
  trafficLightCommand,
  trafficLightShape,
  type TrafficLightCommand,
} from './traffic-lights'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')

describe('traffic light shape', () => {
  const squareCases = [
    { scheme: 'dark', palette: 'charcoal', density: 'compact' },
    { scheme: 'light', palette: 'paper', density: 'compact' },
    { scheme: 'dark', palette: 'charcoal', density: 'regular' },
    { scheme: 'light', palette: 'paper', density: 'regular' },
    { scheme: 'dark', palette: 'charcoal', density: 'spacious' },
    { scheme: 'light', palette: 'paper', density: 'spacious' },
  ] as const

  it('paints squares only for style id square', () => {
    for (const row of squareCases) {
      expect(trafficLightShape('square')).toBe('square')
      expect(
        trafficLightCommand({ styleId: 'square', inset: 0, ...row }).square,
      ).toBe(true)
    }

    for (const styleId of ['glass', 'bezel', 'unknown', 'classic', '', null, undefined]) {
      expect(trafficLightShape(styleId)).toBe('round')
      expect(
        trafficLightCommand({
          styleId,
          inset: 10,
          scheme: 'dark',
          palette: 'obsidian',
          density: 'regular',
        }).square,
      ).toBe(false)
    }
  })

  it('keeps the inset and the 3px nudge for 0, 6, 10, and 12', () => {
    expect(TRAFFIC_LIGHT_Y_NUDGE_PX).toBe(3)
    const rows = [
      { styleId: 'square', scheme: 'dark', palette: 'charcoal', density: 'compact', inset: 0 },
      { styleId: 'square', scheme: 'light', palette: 'paper', density: 'regular', inset: 6 },
      { styleId: 'glass', scheme: 'dark', palette: 'obsidian', density: 'regular', inset: 10 },
      { styleId: 'bezel', scheme: 'light', palette: 'porcelain', density: 'regular', inset: 10 },
      { styleId: 'square', scheme: 'dark', palette: 'charcoal', density: 'spacious', inset: 12 },
      { styleId: 'nope', scheme: 'light', palette: null, density: 'compact', inset: 0 },
    ]
    for (const row of rows) {
      const cmd = trafficLightCommand(row)
      expect(cmd.x).toBe(row.inset)
      expect(cmd.y).toBe(row.inset + 3)
    }
  })
})

describe('traffic light re-apply', () => {
  it('schedules Square compact on resize and fullscreen, and Glass is not square', () => {
    const applied: TrafficLightCommand[] = []
    const scheduled: Array<() => void> = []
    let styleId: string | null = 'square'
    let inset = 0
    const lights = createTrafficLightController({
      isMac: () => true,
      read: () => ({ styleId, inset }),
      apply: (cmd) => {
        applied.push(cmd)
      },
      schedule: (fn) => {
        scheduled.push(fn)
      },
    })

    lights.onResize()
    expect(scheduled).toHaveLength(1)
    scheduled[0]()
    expect(applied).toEqual([{ x: 0, y: 3, square: true }])

    lights.onFullscreen()
    expect(scheduled).toHaveLength(2)
    scheduled[1]()
    expect(applied[1]).toEqual({ x: 0, y: 3, square: true })

    lights.onFullscreen()
    expect(scheduled).toHaveLength(3)
    scheduled[2]()
    expect(applied[2]).toEqual({ x: 0, y: 3, square: true })

    // A burst before the frame runs schedules once.
    const before = scheduled.length
    lights.onResize()
    lights.onFullscreen()
    expect(scheduled).toHaveLength(before + 1)

    styleId = 'glass'
    inset = 10
    lights.syncIfChanged()
    expect(applied.at(-1)).toEqual({ x: 10, y: 13, square: false })

    const glassAt = applied.length
    scheduled.at(-1)!()
    expect(applied[glassAt]).toEqual({ x: 10, y: 13, square: false })
  })
})

describe('traffic light source locks', () => {
  it('does not paint buttons on the non-mac command, or branch WindowControls on data-style', () => {
    const rust = readFileSync(resolve(root, 'src-tauri/src/commands/traffic_lights.rs'), 'utf8')
    const marker = '#[cfg(not(target_os = "macos"))]'
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
    const body = rust.slice(open, end)
    expect(body).not.toMatch(
      /paint|NSButton|setCell|drawWithFrame|standardWindowButton|NSBezierPath|NSButtonCell/,
    )

    const controls = readFileSync(
      resolve(root, 'src/renderer/components/TopBar/WindowControls.tsx'),
      'utf8',
    )
    expect(controls.includes('data-style')).toBe(false)

    const style = readFileSync(resolve(root, 'src/renderer/stores/style.ts'), 'utf8')
    expect(style).toContain("addEventListener('resize'")
    expect(style).toContain("addEventListener('fullscreenchange'")
    expect(style).toContain('onResize')
    expect(style).toContain('onFullscreen')
    expect(style).not.toMatch(/lastTrafficInset\s*<=\s*0/)
  })
})
