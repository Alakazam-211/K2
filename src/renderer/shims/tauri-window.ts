/**
 * Web shim for `@tauri-apps/api/window`.
 */

import { busListen, type EventCallback, type UnlistenFn } from './event-bus'

export interface WebWindow {
  label: string
  listen: <T = unknown>(
    event: string,
    handler: EventCallback<T>,
  ) => Promise<UnlistenFn>
  once: <T = unknown>(
    event: string,
    handler: EventCallback<T>,
  ) => Promise<UnlistenFn>
  emit: <T = unknown>(event: string, payload?: T) => Promise<void>
  isFocused: () => Promise<boolean>
  startDragging: () => Promise<void>
  destroy: () => Promise<void>
  close: () => Promise<void>
  minimize: () => Promise<void>
  maximize: () => Promise<void>
  unmaximize: () => Promise<void>
  toggleMaximize: () => Promise<void>
  show: () => Promise<void>
  hide: () => Promise<void>
  setFocus: () => Promise<void>
  setTitle: (title: string) => Promise<void>
  setAlwaysOnTop: (alwaysOnTop: boolean) => Promise<void>
  setFullscreen: (fullscreen: boolean) => Promise<void>
  isFullscreen: () => Promise<boolean>
  isMaximized: () => Promise<boolean>
  isMinimized: () => Promise<boolean>
  isVisible: () => Promise<boolean>
  scaleFactor: () => Promise<number>
  innerSize: () => Promise<{ width: number; height: number }>
  outerSize: () => Promise<{ width: number; height: number }>
  theme: () => Promise<'light' | 'dark' | null>
}

let focusWired = false

function wireBrowserFocusToTauriEvents(): void {
  if (focusWired || typeof window === 'undefined') return
  focusWired = true
  window.addEventListener('focus', () => {
    const set = (globalThis as { __k2_web_emit?: (e: string) => void })
    void set
    // Use bus via dynamic import avoidance — emit through listen side.
    import('./event-bus').then(({ busEmit }) => {
      busEmit('tauri://focus', null)
    })
  })
  window.addEventListener('blur', () => {
    import('./event-bus').then(({ busEmit }) => {
      busEmit('tauri://blur', null)
    })
  })
}

function makeWindow(label = 'main'): WebWindow {
  wireBrowserFocusToTauriEvents()
  const noop = async (): Promise<void> => {}
  return {
    label,
    async listen<T = unknown>(
      event: string,
      handler: EventCallback<T>,
    ): Promise<UnlistenFn> {
      return busListen(event, handler)
    },
    async once<T = unknown>(
      event: string,
      handler: EventCallback<T>,
    ): Promise<UnlistenFn> {
      const unlisten = busListen<T>(event, (e) => {
        unlisten()
        handler(e)
      })
      return unlisten
    },
    async emit<T = unknown>(event: string, payload?: T): Promise<void> {
      const { busEmit } = await import('./event-bus')
      busEmit(event, payload)
    },
    async isFocused(): Promise<boolean> {
      return typeof document !== 'undefined' ? document.hasFocus() : true
    },
    startDragging: noop,
    destroy: noop,
    close: noop,
    minimize: noop,
    maximize: noop,
    unmaximize: noop,
    toggleMaximize: noop,
    show: noop,
    hide: noop,
    setFocus: noop,
    async setTitle(title: string): Promise<void> {
      if (typeof document !== 'undefined') document.title = title
    },
    setAlwaysOnTop: noop,
    async setFullscreen(fullscreen: boolean): Promise<void> {
      if (typeof document === 'undefined') return
      if (fullscreen) {
        await document.documentElement.requestFullscreen?.().catch(() => {})
      } else if (document.fullscreenElement) {
        await document.exitFullscreen?.().catch(() => {})
      }
    },
    async isFullscreen(): Promise<boolean> {
      return typeof document !== 'undefined' && !!document.fullscreenElement
    },
    async isMaximized(): Promise<boolean> {
      return false
    },
    async isMinimized(): Promise<boolean> {
      return false
    },
    async isVisible(): Promise<boolean> {
      return true
    },
    async scaleFactor(): Promise<number> {
      return typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1
    },
    async innerSize(): Promise<{ width: number; height: number }> {
      return {
        width: typeof window !== 'undefined' ? window.innerWidth : 0,
        height: typeof window !== 'undefined' ? window.innerHeight : 0,
      }
    },
    async outerSize(): Promise<{ width: number; height: number }> {
      return {
        width: typeof window !== 'undefined' ? window.outerWidth : 0,
        height: typeof window !== 'undefined' ? window.outerHeight : 0,
      }
    },
    async theme(): Promise<'light' | 'dark' | null> {
      if (typeof window === 'undefined') return null
      return window.matchMedia?.('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light'
    },
  }
}

const current = makeWindow('main')

export function getCurrentWindow(): WebWindow {
  return current
}

export function getCurrent(): WebWindow {
  return current
}

/** No multi-window support in the web client. */
export async function getAllWindows(): Promise<WebWindow[]> {
  return [current]
}
