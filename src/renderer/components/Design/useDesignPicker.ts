// Design tab picker loop (D-A11).
//
// Select is ON when the Design tab opens (Rosson 2026-10-16); the palette
// button in the chrome bar toggles it. There is no in-page hotkey: Esc in
// the page leaves Select, everything else belongs to the page.
//
// Keys typed into the native page never reach the renderer, so the guest
// script queues picks and Esc and this hook drains the queue by polling the
// Tauri side every ARMED_POLL_MS while Select is on and the tab is shown,
// and not at all otherwise (nothing can arrive then).
//
// The page may not exist yet when the tab opens (the native view is created
// a beat later), so an arm that failed is retried before the next poll
// until one succeeds. The Tauri side remembers Select per view, so a new
// page load (navigation, reload) comes back armed by itself.
//
// `visible` is the TAB's visibility in this window, never the pane's
// native-view visibility (the page goes frozen while the note pop-up is
// open; Select is off then anyway).
//
// A poll carries the page's navigation counter. Picks reported under an
// older counter than one already seen belong to a page that is gone, so
// they are dropped.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { DesignPickerIpc, PickResult } from './types'

export const ARMED_POLL_MS = 120

export interface DesignPickerOptions {
  itemId: string
  parentWindow?: string
  ipc: DesignPickerIpc
  /** The Design tab is shown in this window (NOT "the native view is live"). */
  visible: boolean
  onPick: (pick: PickResult) => void
  /** A poll or arm call failed (logged by the caller; the loop keeps going). */
  onError?: (err: unknown) => void
  /** Select on when the tab opens (default true). */
  initialArmed?: boolean
}

export interface DesignPicker {
  armed: boolean
  setArmed: (on: boolean) => void
  toggle: () => void
}

export function useDesignPicker({
  itemId,
  parentWindow,
  ipc,
  visible,
  onPick,
  onError,
  initialArmed = true,
}: DesignPickerOptions): DesignPicker {
  const [armed, setArmedState] = useState(initialArmed)
  const latestNav = useRef<number>(-Infinity)
  const onPickRef = useRef(onPick)
  onPickRef.current = onPick
  const onErrorRef = useRef(onError)
  onErrorRef.current = onError
  const armedRef = useRef(armed)
  armedRef.current = armed
  /** The page has confirmed the current `armed` value. */
  const syncedRef = useRef(!initialArmed)

  const setArmed = useCallback((on: boolean) => setArmedState(on), [])
  const toggle = useCallback(() => setArmedState((a) => !a), [])

  // Arm / disarm the guest when the state flips. On first mount only an
  // armed start calls (an unarmed page needs nothing).
  const mounted = useRef(false)
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true
      if (!armed) return
    }
    syncedRef.current = false
    const want = armed
    const call = want ? ipc.pickArm(itemId, parentWindow) : ipc.pickDisarm(itemId, parentWindow)
    call.then(
      () => {
        if (armedRef.current === want) syncedRef.current = true
      },
      (e: unknown) => onErrorRef.current?.(e),
    )
  }, [armed, ipc, itemId, parentWindow])

  // Disarm when the tab goes away while Select is on.
  useEffect(() => {
    return () => {
      if (armedRef.current) {
        ipc.pickDisarm(itemId, parentWindow).catch((e: unknown) => onErrorRef.current?.(e))
      }
    }
  }, [ipc, itemId, parentWindow])

  // The poll loop, while Select is on and the tab is shown. One request at
  // a time; the next waits for the last. (While the pop-up is open Select
  // is off, so a frozen page never needs a poll.)
  useEffect(() => {
    if (!armed || !visible) return
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | null = null

    const tick = async (): Promise<void> => {
      timer = null
      try {
        // An arm that failed (no page yet) is retried until one lands.
        if (!syncedRef.current) {
          await ipc.pickArm(itemId, parentWindow)
          if (stopped) return
          syncedRef.current = true
        }
        const poll = await ipc.pickPoll(itemId, parentWindow)
        if (stopped || !poll) return
        const stale = poll.navSeq < latestNav.current
        if (poll.navSeq > latestNav.current) latestNav.current = poll.navSeq
        for (const ev of poll.events) {
          if (ev.type === 'key') {
            if (ev.key === 'escape') setArmedState(false)
          } else if (ev.type === 'pick' && !stale) {
            onPickRef.current(ev)
          }
        }
      } catch (e) {
        if (!stopped) onErrorRef.current?.(e)
      } finally {
        if (!stopped) timer = setTimeout(() => void tick(), ARMED_POLL_MS)
      }
    }
    timer = setTimeout(() => void tick(), ARMED_POLL_MS)
    return () => {
      stopped = true
      if (timer !== null) clearTimeout(timer)
    }
  }, [armed, visible, ipc, itemId, parentWindow])

  return { armed, setArmed, toggle }
}
