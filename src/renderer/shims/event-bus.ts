/**
 * Shared in-memory event bus for the hosted web client Tauri shims.
 * Mirrors enough of Tauri's event surface for listen/emit/window.listen.
 */

export type EventCallback<T = unknown> = (event: { event: string; payload: T; id: number }) => void
export type UnlistenFn = () => void

const listeners = new Map<string, Set<EventCallback>>()
let nextEventId = 1

export function busListen<T = unknown>(
  event: string,
  handler: EventCallback<T>,
): UnlistenFn {
  let set = listeners.get(event)
  if (!set) {
    set = new Set()
    listeners.set(event, set)
  }
  const wrapped = handler as EventCallback
  set.add(wrapped)
  return () => {
    set!.delete(wrapped)
    if (set!.size === 0) listeners.delete(event)
  }
}

export function busEmit<T = unknown>(event: string, payload?: T): void {
  const set = listeners.get(event)
  if (!set || set.size === 0) return
  const id = nextEventId++
  for (const handler of [...set]) {
    try {
      handler({ event, payload: payload as T, id })
    } catch (err) {
      console.warn(`[web-shim] event handler error for '${event}':`, err)
    }
  }
}

export function busOnce<T = unknown>(
  event: string,
  handler: EventCallback<T>,
): UnlistenFn {
  const unlisten = busListen<T>(event, (e) => {
    unlisten()
    handler(e)
  })
  return unlisten
}
