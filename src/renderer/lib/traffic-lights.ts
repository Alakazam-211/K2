// macOS stoplight shape + inset. Pure: no AppKit, no DOM.
//
// Only style id `square` paints squares. Scheme, palette, and density
// do not turn that off. The y nudge is the title-bar grow the native
// command already applies; it is not added to the button origin again.

export const TRAFFIC_LIGHT_Y_NUDGE_PX = 3

export type TrafficLightShape = 'square' | 'round'

export type TrafficLightCommand = {
  x: number
  y: number
  square: boolean
  /**
   * App zoom (`documentElement.style.zoom`, 1 = 100%). CSS zoom scales the
   * top bar but not the native buttons, so Rust re-centers them on it.
   */
  zoom: number
}

/** A finite zoom above 0, else 1. */
export function trafficLightZoom(zoom: number | null | undefined): number {
  return typeof zoom === 'number' && Number.isFinite(zoom) && zoom > 0 ? zoom : 1
}

/** `square` only. A missing or unknown id is round. */
export function trafficLightShape(styleId: string | null | undefined): TrafficLightShape {
  return styleId === 'square' ? 'square' : 'round'
}

/** x is the window inset. y is inset + the 3px title-bar nudge. */
export function trafficLightOffsets(inset: number): { x: number; y: number } {
  return { x: inset, y: inset + TRAFFIC_LIGHT_Y_NUDGE_PX }
}

/**
 * Shape ignores scheme, palette, and density (Square compact / regular /
 * spacious, Paper and Charcoal). Inset math does not change with shape.
 */
export function trafficLightCommand(input: {
  styleId: string | null | undefined
  inset: number
  scheme?: string | null
  palette?: string | null
  density?: string | null
  zoom?: number | null
}): TrafficLightCommand {
  const { x, y } = trafficLightOffsets(input.inset)
  return {
    x,
    y,
    square: trafficLightShape(input.styleId) === 'square',
    zoom: trafficLightZoom(input.zoom),
  }
}

export type TrafficLightController = {
  /** Invoke only when the inset, zoom, or square-vs-round bit changed. */
  syncIfChanged: () => void
  /** Invoke with the current inset and shape, even when inset is 0. */
  reapply: () => void
  /** Forget the last invoke so the next sync sends again. */
  resetBaseline: () => void
  /** Resize. Square compact (inset 0) still schedules a re-apply. */
  onResize: () => void
  /** Fullscreen enter and exit share this. Both schedule a re-apply. */
  onFullscreen: () => void
}

export function createTrafficLightController(opts: {
  isMac: () => boolean
  read: () => { styleId: string | null | undefined; inset: number; zoom?: number | null }
  apply: (cmd: TrafficLightCommand) => void
  schedule: (fn: () => void) => void
}): TrafficLightController {
  let insetSeen: number | null = null
  let squareSeen: boolean | null = null
  let zoomSeen: number | null = null
  let queued = false

  function emit(cmd: TrafficLightCommand, inset: number): void {
    insetSeen = inset
    squareSeen = cmd.square
    zoomSeen = cmd.zoom
    opts.apply(cmd)
  }

  function reapply(): void {
    if (!opts.isMac()) return
    const ctx = opts.read()
    emit(trafficLightCommand(ctx), ctx.inset)
  }

  function syncIfChanged(): void {
    if (!opts.isMac()) return
    const ctx = opts.read()
    const cmd = trafficLightCommand(ctx)
    if (ctx.inset === insetSeen && cmd.square === squareSeen && cmd.zoom === zoomSeen) return
    emit(cmd, ctx.inset)
  }

  function resetBaseline(): void {
    insetSeen = null
    squareSeen = null
    zoomSeen = null
  }

  function scheduleReapply(): void {
    if (!opts.isMac()) return
    if (queued) return
    queued = true
    opts.schedule(() => {
      queued = false
      reapply()
    })
  }

  return {
    syncIfChanged,
    reapply,
    resetBaseline,
    // Do not skip inset 0. AppKit parks the buttons on a title-bar
    // reset even when the only vertical extra is the 3px nudge.
    onResize: scheduleReapply,
    onFullscreen: scheduleReapply,
  }
}
