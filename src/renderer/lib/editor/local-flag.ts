// One on/off preference kept on this computer, in this window's storage ('1'
// on, '0' off, missing = the default), shared by every window through the
// `storage` event. For view preferences only (the editor's preview switch,
// Full width, Wrap in markdown): nothing here is the daemon's truth.

import { useSyncExternalStore } from 'react'

const listeners = new Set<() => void>()
/** Blocked storage keeps the flags for this window only. */
const memory = new Map<string, boolean>()

export function readLocalFlag(key: string, fallback = false): boolean {
  try {
    const v = window.localStorage.getItem(key)
    return v === null ? fallback : v === '1'
  } catch {
    return memory.get(key) ?? fallback
  }
}

/** Off for a default-off flag removes the key, so a missing key stays "off". */
export function setLocalFlag(key: string, on: boolean, fallback = false): void {
  try {
    if (on === fallback) window.localStorage.removeItem(key)
    else window.localStorage.setItem(key, on ? '1' : '0')
  } catch {
    memory.set(key, on)
  }
  for (const l of listeners) l()
}

export function subscribeLocalFlags(cb: () => void): () => void {
  listeners.add(cb)
  const onStorage = (): void => cb()
  window.addEventListener('storage', onStorage)
  return () => {
    listeners.delete(cb)
    window.removeEventListener('storage', onStorage)
  }
}

export function useLocalFlag(key: string, fallback = false): [boolean, (on: boolean) => void] {
  const on = useSyncExternalStore(subscribeLocalFlags, () => readLocalFlag(key, fallback), () => fallback)
  return [on, (next: boolean) => setLocalFlag(key, next, fallback)]
}
