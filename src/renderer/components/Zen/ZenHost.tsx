// prd-zen-mode-v1 Z6, Z30, Z53, Z55, Z60 — Zen's host, mounted by
// ConnectionGate beside `HomeRoomsHost` as an always-present child of every
// gate branch, OUTSIDE the keyed `<App>`: a server switch never remounts
// Zen. It lives in the App chunk (read lazily by the gate), so its stores
// evaluate only after the gate accepted a daemon.
//
// Always mounted (in a window where Zen exists):
//   - the escape hatch: the `menu:zen-toggle` event (macOS native menu,
//     targeted at this window; the Linux / Windows app menu, a DOM event)
//     and, on Linux / Windows only, the Ctrl+Alt+Z capture listener;
//   - the macOS menu label (Enter / Exit Zen Mode) for the focused window;
//   - `homes.json` upkeep (`POST /cli/zen/homes/sync`) on Home create,
//     rename and delete, once this computer has used Zen;
//   - leaving Zen ends safe mode (Z29).
// The Zen layer itself renders only while this window shows Zen.

import { useEffect, useMemo, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useHomesStore } from '@/stores/homes'
import { useWindowFocusStore } from '@/stores/window-focus'
import { anyZenHomeOn } from '@/lib/zen/zen-homes'
import { syncZenHomes } from '@/lib/zen/zen-api'
import { currentDesktopOs, zenAvailable } from '@/lib/zen/zen-platform'
import { installZenChordListener, ZEN_MENU_EVENT } from '@/lib/zen/zen-shortcut'
import { toggleZenFromEscape, useZenShown, useZenViewStore } from '@/lib/zen/zen-view'
import { ZenRoot } from './ZenRoot'

/** The escape hatch's listeners (Z30). Never inside the Zen root, so no
 *  widget can swallow them. */
function useZenEscapeHatch(): void {
  useEffect(() => {
    const os = currentDesktopOs()
    const onMenu = (): void => toggleZenFromEscape()
    // Linux / Windows app menu (same event name, this window only).
    window.addEventListener(ZEN_MENU_EVENT, onMenu)
    // macOS native menu: emitted to the focused window's label only.
    let unlisten: (() => void) | null = null
    let dead = false
    let label: string | null = null
    try {
      label = getCurrentWindow().label
    } catch {
      label = null
    }
    if (label) {
      void import('@tauri-apps/api/event')
        .then(({ listen }) => listen(ZEN_MENU_EVENT, onMenu, { target: { kind: 'AnyLabel', label: label as string } }))
        .then((fn) => {
          if (dead) fn()
          else unlisten = fn
        })
        .catch((err: unknown) => console.warn('[zen] menu listen failed:', err))
    }
    // Linux / Windows chord owner; null on macOS (the accelerator owns it).
    const uninstallChord = installZenChordListener(os, window, toggleZenFromEscape)
    return () => {
      dead = true
      window.removeEventListener(ZEN_MENU_EVENT, onMenu)
      unlisten?.()
      uninstallChord?.()
    }
  }, [])
}

/** macOS: the View menu says Exit Zen Mode while the focused window shows
 *  Zen, else Enter Zen Mode. */
function useZenMenuLabel(shown: boolean): void {
  const focused = useWindowFocusStore((s) => s.isFocused)
  const isMac = useMemo(() => currentDesktopOs() === 'mac', [])
  useEffect(() => {
    if (!isMac || !focused) return
    void invoke('set_zen_menu_label', { inZen: shown }).catch(() => {})
  }, [isMac, focused, shown])
}

/** Z11: keep `homes.json` (id → name) current once Zen is in use here. */
function useZenHomesSync(): void {
  const homes = useHomesStore((s) => s.homes)
  const key = homes.map((h) => `${h.id}\u0000${h.name}`).join('\u0001')
  const first = useRef(true)
  useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    if (!anyZenHomeOn()) return
    const t = setTimeout(() => {
      void syncZenHomes(useHomesStore.getState().homes).catch((err: unknown) =>
        console.warn('[zen] homes/sync failed:', err),
      )
    }, 300)
    return () => clearTimeout(t)
  }, [key])
}

function ZenHostInner(): React.JSX.Element | null {
  const shown = useZenShown()
  useZenEscapeHatch()
  useZenMenuLabel(shown)
  useZenHomesSync()
  // Leaving Zen (exit, Settings, another page, another Home) ends safe mode.
  useEffect(() => {
    if (!shown && useZenViewStore.getState().safe) useZenViewStore.setState({ safe: null })
  }, [shown])
  return shown ? <ZenRoot /> : null
}

export function ZenHost(): React.JSX.Element | null {
  if (!zenAvailable()) return null
  return <ZenHostInner />
}
