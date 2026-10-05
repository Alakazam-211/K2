import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  STOPLIGHT_SPACER_VAR,
  TRAFFIC_LIGHT_Y_NUDGE_PX,
  ZEN_STOPLIGHT_INSET_PX,
  createTrafficLightController,
  stoplightSpacerPx,
  trafficLightCommand,
  trafficLightShape,
  trafficLightZoom,
  type TrafficLightCommand,
} from './traffic-lights'

import {
  TOP_BAR_PAD_X_PX,
  TRAFFIC_LIGHT_CLUSTER_GAP_PX,
  TRAFFIC_LIGHT_CLUSTER_RIGHT_PX,
  TRAFFIC_LIGHT_SPACER_BASE_PX,
} from './desktop-chrome'

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
    expect(applied).toEqual([{ x: 0, y: 3, square: true, zoom: 1 }])

    lights.onFullscreen()
    expect(scheduled).toHaveLength(2)
    scheduled[1]()
    expect(applied[1]).toEqual({ x: 0, y: 3, square: true, zoom: 1 })

    lights.onFullscreen()
    expect(scheduled).toHaveLength(3)
    scheduled[2]()
    expect(applied[2]).toEqual({ x: 0, y: 3, square: true, zoom: 1 })

    // A burst before the frame runs schedules once.
    const before = scheduled.length
    lights.onResize()
    lights.onFullscreen()
    expect(scheduled).toHaveLength(before + 1)

    styleId = 'glass'
    inset = 10
    lights.syncIfChanged()
    expect(applied.at(-1)).toEqual({ x: 10, y: 13, square: false, zoom: 1 })

    const glassAt = applied.length
    scheduled.at(-1)!()
    expect(applied[glassAt]).toEqual({ x: 10, y: 13, square: false, zoom: 1 })
  })

  it('sends the app zoom and re-sends when only the zoom changes', () => {
    const applied: TrafficLightCommand[] = []
    let zoom: number | undefined = undefined
    const lights = createTrafficLightController({
      isMac: () => true,
      read: () => ({ styleId: 'square', inset: 0, zoom }),
      apply: (cmd) => {
        applied.push(cmd)
      },
      schedule: (fn) => fn(),
    })

    lights.syncIfChanged()
    expect(applied).toEqual([{ x: 0, y: 3, square: true, zoom: 1 }])

    lights.syncIfChanged()
    expect(applied).toHaveLength(1)

    zoom = 1.2
    lights.syncIfChanged()
    expect(applied).toHaveLength(2)
    expect(applied[1]).toEqual({ x: 0, y: 3, square: true, zoom: 1.2 })

    zoom = 1.2
    lights.syncIfChanged()
    expect(applied).toHaveLength(2)

    lights.reapply()
    expect(applied[2]).toEqual({ x: 0, y: 3, square: true, zoom: 1.2 })
  })

  it('falls back to zoom 1 for a missing or bad zoom', () => {
    expect(trafficLightZoom(undefined)).toBe(1)
    expect(trafficLightZoom(null)).toBe(1)
    expect(trafficLightZoom(0)).toBe(1)
    expect(trafficLightZoom(-1)).toBe(1)
    expect(trafficLightZoom(Number.NaN)).toBe(1)
    expect(trafficLightZoom(Number.POSITIVE_INFINITY)).toBe(1)
    expect(trafficLightZoom(1.5)).toBe(1.5)
    expect(trafficLightCommand({ styleId: 'glass', inset: 10, zoom: 2 })).toEqual({
      x: 10,
      y: 13,
      square: false,
      zoom: 2,
    })
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

describe('stoplight spacer under app zoom', () => {
  // The zoom levels Cmd+- / Cmd+= step through that Rosson checked.
  const expected: Array<[number, number]> = [
    [0.8, 69 / 0.8 - 12],
    [1.0, 57],
    [1.25, 69 / 1.25 - 12],
    [1.5, 69 / 1.5 - 12],
  ]

  it('is the native cluster width divided by the zoom at inset 0', () => {
    expect(TRAFFIC_LIGHT_CLUSTER_RIGHT_PX).toBe(69)
    expect(TOP_BAR_PAD_X_PX).toBe(12)
    for (const [zoom, want] of expected) {
      const got = stoplightSpacerPx(zoom, 0)
      expect(got, `zoom ${zoom}`).toBeCloseTo(want, 9)
      // On screen: the zoomed px-3 plus the zoomed spacer end exactly on
      // the unscaled zoom light's right edge.
      expect((TOP_BAR_PAD_X_PX + got) * zoom, `zoom ${zoom}`).toBeCloseTo(69, 9)
    }
    expect(stoplightSpacerPx(1, 0)).toBe(TRAFFIC_LIGHT_SPACER_BASE_PX)
    // Zooming out widens the spacer, zooming in narrows it.
    expect(stoplightSpacerPx(0.8, 0)).toBeGreaterThan(stoplightSpacerPx(1, 0))
    expect(stoplightSpacerPx(1.5, 0)).toBeLessThan(stoplightSpacerPx(1.25, 0))
  })

  it('keeps the buttons on the unscaled window inset while the DOM inset zooms', () => {
    for (const inset of [6, 10, 12]) {
      for (const [zoom] of expected) {
        const got = stoplightSpacerPx(zoom, inset)
        // Rust moves the buttons right by the raw inset (not zoomed).
        const nativeRight = TRAFFIC_LIGHT_CLUSTER_RIGHT_PX + inset
        expect((inset + TOP_BAR_PAD_X_PX + got) * zoom, `inset ${inset} zoom ${zoom}`).toBeCloseTo(
          nativeRight,
          9,
        )
      }
      expect(stoplightSpacerPx(1, inset)).toBe(TRAFFIC_LIGHT_SPACER_BASE_PX)
    }
  })

  it('falls back to 100% for a bad zoom and never goes negative', () => {
    for (const zoom of [undefined, null, 0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(stoplightSpacerPx(zoom, 0)).toBe(TRAFFIC_LIGHT_SPACER_BASE_PX)
    }
    expect(stoplightSpacerPx(1, Number.NaN)).toBe(TRAFFIC_LIGHT_SPACER_BASE_PX)
    expect(stoplightSpacerPx(10, 0)).toBe(0)
  })

  it('the zoom hook re-sends the zoom now and writes the spacer with it', () => {
    const applied: TrafficLightCommand[] = []
    const spacers: number[] = []
    const scheduled: Array<() => void> = []
    let zoom = 1
    const lights = createTrafficLightController({
      isMac: () => true,
      read: () => ({ styleId: 'square', inset: 0, zoom }),
      apply: (cmd) => {
        applied.push(cmd)
      },
      setSpacer: (px) => {
        spacers.push(px)
      },
      schedule: (fn) => {
        scheduled.push(fn)
      },
    })

    for (const [z, want] of expected) {
      zoom = z
      lights.onZoomChange()
      expect(applied.at(-1)).toEqual({ x: 0, y: 3, square: true, zoom: z })
      expect(spacers.at(-1)).toBeCloseTo(want, 9)
    }
    expect(applied).toHaveLength(expected.length)
    expect(spacers).toHaveLength(expected.length)
    // Immediate, not deferred to the next frame.
    expect(scheduled).toHaveLength(0)

    // Same zoom again still re-applies (AppKit may have parked the buttons).
    lights.onZoomChange()
    expect(applied).toHaveLength(expected.length + 1)

    // Every apply path writes the spacer, so it cannot go stale.
    lights.resetBaseline()
    lights.syncIfChanged()
    expect(spacers).toHaveLength(applied.length)
  })

  it('does nothing off macOS', () => {
    const applied: TrafficLightCommand[] = []
    const spacers: number[] = []
    const scheduled: Array<() => void> = []
    const lights = createTrafficLightController({
      isMac: () => false,
      read: () => ({ styleId: 'square', inset: 0, zoom: 1.5 }),
      apply: (cmd) => {
        applied.push(cmd)
      },
      setSpacer: (px) => {
        spacers.push(px)
      },
      schedule: (fn) => {
        scheduled.push(fn)
      },
    })
    lights.onZoomChange()
    lights.reapply()
    lights.syncIfChanged()
    lights.onResize()
    lights.onFullscreen()
    expect(applied).toEqual([])
    expect(spacers).toEqual([])
    expect(scheduled).toEqual([])
  })

  it('globals.css holds the 100% width and every stoplight spacer reads it', () => {
    const css = readFileSync(resolve(root, 'src/renderer/globals.css'), 'utf8')
    expect(STOPLIGHT_SPACER_VAR).toBe('--k2-stoplight-spacer')
    expect(css).toContain(`${STOPLIGHT_SPACER_VAR}: ${TRAFFIC_LIGHT_SPACER_BASE_PX}px;`)
    expect(css).toMatch(/\.k2-stoplight-spacer \{[^}]*width: var\(--k2-stoplight-spacer\);/)
    // gap-3 headers: the cluster gap (14) minus their gap (12).
    expect(TRAFFIC_LIGHT_CLUSTER_GAP_PX - 12).toBe(2)
    expect(css).toMatch(
      /\.k2-stoplight-spacer-brief \{[^}]*width: calc\(var\(--k2-stoplight-spacer\) \+ 2px\);/,
    )

    const read = (rel: string): string => readFileSync(resolve(root, rel), 'utf8')
    const spacer = read('src/renderer/components/TopBar/TrafficLightSpacer.tsx')
    expect(spacer).toContain('className="k2-stoplight-spacer"')
    expect(spacer).not.toMatch(/style=\{\{\s*width/)
    for (const rel of [
      'src/renderer/components/Feedback/BriefFrame.tsx',
      'src/renderer/components/Feedback/TicketWindow.tsx',
    ]) {
      const src = read(rel)
      expect(src, rel).toContain('className="k2-stoplight-spacer-brief"')
      expect(src, rel).not.toContain('stoplightInset - 12 }')
    }
  })

  it('App zoom calls the reposition hook, which writes the spacer variable', () => {
    const app = readFileSync(resolve(root, 'src/renderer/App.tsx'), 'utf8')
    const start = app.indexOf('function applyK2SOZoom(): void {')
    expect(start).toBeGreaterThan(-1)
    const end = app.indexOf('\n}\n', start)
    expect(end).toBeGreaterThan(start)
    const body = app.slice(start, end)
    expect(body).toContain('onAppZoomChange()')
    // After the zoom is on <html>, so the spacer and the bar move together.
    expect(body.indexOf('onAppZoomChange()')).toBeGreaterThan(
      body.indexOf('document.documentElement.style.zoom = String(z)'),
    )
    // Startup: the mount effect runs it.
    expect(app).toMatch(/useEffect\(\(\) => \{\s*applyK2SOZoom\(\)/)

    const style = readFileSync(resolve(root, 'src/renderer/stores/style.ts'), 'utf8')
    expect(style).toContain('export function onAppZoomChange(): void')
    expect(style).toContain('trafficLights.onZoomChange()')
    expect(style).toContain('setProperty(STOPLIGHT_SPACER_VAR')
  })
})

describe('Zen stoplights (Rosson 2026-10-04: down and right in Zen)', () => {
  it('Zen adds its own 8px inset right and down on top of the theme offset; the Style never does', () => {
    expect(ZEN_STOPLIGHT_INSET_PX).toBe(8)
    const zen = trafficLightCommand({ styleId: 'square', inset: 0, zoom: 1.25, zen: { square: false, x: 0, y: 0 } })
    expect(zen).toEqual({ x: 8, y: 8 + TRAFFIC_LIGHT_Y_NUDGE_PX, square: false, zoom: 1.25 })
    const nudged = trafficLightCommand({ styleId: null, inset: 0, zen: { square: true, x: 6, y: 4 } })
    expect(nudged).toEqual({ x: 14, y: 15, square: true, zoom: 1 })
    // Leaving Zen (no `zen`): the Style's own command, no Zen inset.
    expect(trafficLightCommand({ styleId: 'square', inset: 0, zoom: 1.25 })).toEqual({ x: 0, y: 3, square: true, zoom: 1.25 })
  })
})
