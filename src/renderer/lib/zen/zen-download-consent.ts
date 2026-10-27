// `k2.download` (prd-garden-board-needs-v1 GS1; security review #1, belt
// and braces): K2's own card the first time a widget saves a file to
// Downloads in a Garden. "Always" is kept per Garden and widget on this
// computer's app (this window's memory and its local storage); "Save once"
// asks again next time. The press check and the hourly budget still apply
// to every download.

/** The person's answer on K2's card. */
export type ZenDownloadAnswer = 'deny' | 'once' | 'always'

/** What K2's card shows (all K2's words; the widget gave only the name). */
export interface ZenDownloadAsk {
  widget: string
  garden: string
  name: string
  /** Bytes. */
  size: number
}

const STORAGE_KEY = 'k2.zen.downloadsAllowed'
const allowed = new Set<string>()

function stored(): string[] {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY)
    const v: unknown = raw ? JSON.parse(raw) : []
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}

/** The Garden/widget pair a consent covers. */
export function zenDownloadConsentKey(gardenId: string, widget: string): string {
  return `${gardenId}/${widget}`
}

export function zenDownloadAllowed(key: string): boolean {
  return allowed.has(key) || stored().includes(key)
}

export function rememberZenDownloadAllowed(key: string): void {
  allowed.add(key)
  try {
    const all = new Set(stored())
    all.add(key)
    globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify([...all]))
  } catch (err) {
    // This window still remembers it; the next window asks again.
    console.warn('[zen] k2.download: keeping the answer failed:', err)
  }
}

/** Tests only. */
export function __resetZenDownloadConsentForTests(): void {
  allowed.clear()
  try {
    globalThis.localStorage?.removeItem(STORAGE_KEY)
  } catch {
    // nothing kept
  }
}

/** Ask (or not) and answer whether to go on. */
export async function zenDownloadConsent(
  key: string,
  ask: ZenDownloadAsk,
  show: (ask: ZenDownloadAsk) => Promise<ZenDownloadAnswer>,
): Promise<boolean> {
  if (zenDownloadAllowed(key)) return true
  const answer = await show(ask)
  if (answer === 'always') rememberZenDownloadAllowed(key)
  return answer !== 'deny'
}
