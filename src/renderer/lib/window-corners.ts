// macOS window clip for style id `square`. Pure: no AppKit, no DOM.
//
// 0.5 is the measured radius whose corner pixel is opaque at 2×.
// 0 tells AppKit to restore the system squircle — it does not square
// the window. Scheme, palette, gaps, and the traffic-light inset do
// not change the number. Square compact (inset 0) is still 0.5.

export type WindowCornerInput = {
  styleId: string | null | undefined
  scheme?: string | null
  palette?: string | null
  gaps?: string | null
  density?: string | null
  /** Ignored. Present so inset 0 cannot be mistaken for "skip". */
  inset?: number
}

/** `square` is 0.5. Any other id, including unknown, is 0. */
export function windowCornerRadius(input: WindowCornerInput): number {
  return input.styleId === 'square' ? 0.5 : 0
}

export type WindowCornerController = {
  /** Push the current style's radius now. */
  reapply: () => void
  /** Resize. Square compact (inset 0) still schedules a re-apply. */
  onResize: () => void
  /** Fullscreen enter and exit share this. Both schedule a re-apply. */
  onFullscreen: () => void
}

export function createWindowCornerController(opts: {
  isMac: () => boolean
  read: () => WindowCornerInput
  apply: (radius: number) => void
  schedule: (fn: () => void) => void
}): WindowCornerController {
  let queued = false

  function reapply(): void {
    if (!opts.isMac()) return
    opts.apply(windowCornerRadius(opts.read()))
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
    reapply,
    // Inset is not an input. Square compact still schedules.
    onResize: scheduleReapply,
    onFullscreen: scheduleReapply,
  }
}
