// The Mobile tab's per-viewer view choices (research-mobile-pane-polish-v1
// §5: view preferences live in this browser, never in the daemon): Frame
// or Screen only, and live touch (§4 item 3, behind a switch until the iOS
// round-trip is measured). Storage can be missing or throw (private
// windows, previews); every read falls back to the default.

const SCREEN_ONLY_KEY = 'k2.mobile.screenOnly'
const LIVE_TOUCH_KEY = 'k2.mobile.liveTouch'

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

/** Live touch (down / move / up while dragging). Off by default until the
 *  iOS lane's round-trip at 60 Hz is measured; `localStorage
 *  k2.mobile.liveTouch = '1'` turns it on. */
export function readMobileLiveTouch(): boolean {
  return read(LIVE_TOUCH_KEY) === '1'
}

export function writeMobileLiveTouch(on: boolean): void {
  write(LIVE_TOUCH_KEY, on ? '1' : null)
}
