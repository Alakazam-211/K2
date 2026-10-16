// Design tab picker loop (D-A11).
//
// Keys typed into the native page never reach the renderer, so the guest
// script queues them (S toggles Select, Esc leaves it) and this hook drains
// the queue by polling the Tauri side:
//  - every ARMED_POLL_MS while Select is on (picks + keys),
//  - every IDLE_POLL_MS while the Design tab is shown but Select is off
//    (only the S key can arrive then),
//  - not at all while the tab is hidden and Select is off.
//
// `visible` is the TAB's visibility in this window, never the pane's
// native-view visibility: the page goes frozen (picture shown, native view
// hidden) while the note pop-up is open, and polling must go on then.
//
// A poll carries the page's navigation counter. Picks reported under an
// older counter than one already seen belong to a page that is gone, so
// they are dropped.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { DesignPickerIpc, PickResult } from './types'

export const ARMED_POLL_MS = 120
export const IDLE_POLL_MS = 400

export interface DesignPickerOptions {
  itemId: string
  parentWindow?: string
  ipc: DesignPickerIpc
  /** The Design tab is shown in this window (NOT "the native view is live"). */
  visible: boolean
  onPick: (pick: PickResult) => void
  /** A poll or arm call failed (logged by the caller; the loop keeps going). */
  onError?: (err: unknown) => void
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
}: DesignPickerOptions): DesignPicker {
  const [armed, setArmedState] = useState(false)
  const latestNav = useRef<number>(-Infinity)
  const onPickRef = useRef(onPick)
  onPickRef.current = onPick
  const onErrorRef = useRef(onError)
  onErrorRef.current = onError
  const armedRef = useRef(false)
  armedRef.current = armed

  const setArmed = useCallback((on: boolean) => setArmedState(on), [])
  const toggle = useCallback(() => setArmedState((a) => !a), [])

  // Arm / disarm the guest when the state flips (not on first mount).
  const mounted = useRef(false)
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true
      if (!armed) return
    }
    const call = armed ? ipc.pickArm(itemId, parentWindow) : ipc.pickDisarm(itemId, parentWindow)
    call.catch((e: unknown) => onErrorRef.current?.(e))
  }, [armed, ipc, itemId, parentWindow])

  // Disarm when the tab goes away while Select is on.
  useEffect(() => {
    return () => {
      if (armedRef.current) {
        ipc.pickDisarm(itemId, parentWindow).catch((e: unknown) => onErrorRef.current?.(e))
      }
    }
  }, [ipc, itemId, parentWindow])

  // The poll loop. One request at a time; the next waits for the last.
  useEffect(() => {
    const interval = armed ? ARMED_POLL_MS : visible ? IDLE_POLL_MS : 0
    if (interval === 0) return
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | null = null

    const tick = async (): Promise<void> => {
      timer = null
      try {
        const poll = await ipc.pickPoll(itemId, parentWindow)
        if (stopped || !poll) return
        const stale = poll.navSeq < latestNav.current
        if (poll.navSeq > latestNav.current) latestNav.current = poll.navSeq
        for (const ev of poll.events) {
          if (ev.type === 'key') {
            if (ev.key === 'toggle-select') setArmedState((a) => !a)
            else if (ev.key === 'escape') setArmedState(false)
          } else if (ev.type === 'pick' && !stale) {
            onPickRef.current(ev)
          }
        }
      } catch (e) {
        if (!stopped) onErrorRef.current?.(e)
      } finally {
        if (!stopped) timer = setTimeout(() => void tick(), interval)
      }
    }
    timer = setTimeout(() => void tick(), interval)
    return () => {
      stopped = true
      if (timer !== null) clearTimeout(timer)
    }
  }, [armed, visible, ipc, itemId, parentWindow])

  return { armed, setArmed, toggle }
}
