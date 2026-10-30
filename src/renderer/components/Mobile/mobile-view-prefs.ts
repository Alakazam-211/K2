// The Mobile tab's per-viewer view choices (research-mobile-pane-polish-v1
// §5: view preferences live in this browser, never in the daemon): Frame
// or Screen only, live touch, and the latency readout. Storage can be
// missing or throw (private windows, previews); every read falls back to
// the default.

const SCREEN_ONLY_KEY = 'k2.mobile.screenOnly'
const LIVE_TOUCH_KEY = 'k2.mobile.liveTouch'
const LATENCY_HUD_KEY = 'k2.mobile.latencyHud'

function read(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function write(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key)
    else localStorage.setItem(key, value)
  } catch {
    // Storage unavailable: the choice lasts this session only.
  }
}

export function readMobileScreenOnly(): boolean {
  return read(SCREEN_ONLY_KEY) === '1'
}

export function writeMobileScreenOnly(on: boolean): void {
  write(SCREEN_ONLY_KEY, on ? '1' : null)
}

/** Live touch (down / move / up while dragging): on by default since
 *  0.46.7 (Rosson 2026-10-30). Stored as `'1'` on / `'0'` off; no key is
 *  the default. Before 0.46.7, off removed the key, so that old off reads
 *  as the default now; the `'0'` an explicit off writes today stays off. */
export function readMobileLiveTouch(): boolean {
  return read(LIVE_TOUCH_KEY) !== '0'
}

export function writeMobileLiveTouch(on: boolean): void {
  write(LIVE_TOUCH_KEY, on ? '1' : '0')
}

/** The input → frame latency readout over the device. Off by default:
 *  it's for measuring. */
export function readMobileLatencyHud(): boolean {
  return read(LATENCY_HUD_KEY) === '1'
}

export function writeMobileLatencyHud(on: boolean): void {
  write(LATENCY_HUD_KEY, on ? '1' : null)
}
