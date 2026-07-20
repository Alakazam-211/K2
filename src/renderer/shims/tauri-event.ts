/**
 * Web shim for `@tauri-apps/api/event`.
 */

import {
  busEmit,
  busListen,
  busOnce,
  type EventCallback,
  type UnlistenFn,
} from './event-bus'

export type { EventCallback, UnlistenFn }

export async function listen<T = unknown>(
  event: string,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return busListen(event, handler)
}

export async function once<T = unknown>(
  event: string,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return busOnce(event, handler)
}

export async function emit<T = unknown>(
  event: string,
  payload?: T,
): Promise<void> {
  busEmit(event, payload)
}

export async function emitTo<T = unknown>(
  _target: unknown,
  event: string,
  payload?: T,
): Promise<void> {
  busEmit(event, payload)
}
