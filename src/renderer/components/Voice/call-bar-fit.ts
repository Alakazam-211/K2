// The call's slim bar in a top bar too narrow for it (an 800 px window, the
// app zoomed in) gives way a step at a time, so the top bar never runs past
// the window's edge and hides what follows it (Usage, the clock, Help…):
//
//   no-name  the call's name goes (the region still names it), and so does
//            "In its own window";
//   no-time  the time goes too;
//   icons    the action (Back to call, Show call here…) shows its icon,
//            its word in the tooltip and its name;
//   minimal  the last resort: mute, hang up, the recording badge and the
//            action only (deafen, Record and Stop,
//            noise removal, camera and share are in the headset's list).
//
// The slim bar carries the step in `data-call-bar-fit`; globals.css hides
// the parts. Each pass starts from the full bar and steps down until the
// whole top bar fits its width (scrollWidth against clientWidth, CSS px, so
// it is the same at any zoom). It folds before anything else in the bar.

import { useLayoutEffect, useRef } from 'react'

export const CALL_BAR_FIT_STEPS = ['full', 'no-name', 'no-time', 'icons', 'minimal'] as const
export type CallBarFit = (typeof CALL_BAR_FIT_STEPS)[number]

/** Whether the top bar holds all of its items without overflowing. */
export function topBarHoldsAll(bar: HTMLElement): boolean {
  return bar.scrollWidth <= bar.clientWidth
}

/** Steps the slim bar down until the top bar fits (or the last step). */
export function fitCallBar(bar: HTMLElement, callBar: HTMLElement): CallBarFit {
  for (const step of CALL_BAR_FIT_STEPS) {
    callBar.dataset.callBarFit = step
    if (topBarHoldsAll(bar)) return step
  }
  return CALL_BAR_FIT_STEPS[CALL_BAR_FIT_STEPS.length - 1]
}

/** A ref for the slim bar; keeps `data-call-bar-fit` current as the window,
 *  the zoom or the top bar's items change. Off (`full`) outside a top bar. */
export function useCallBarFit(on: boolean): React.RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const call = ref.current
    if (!on || !call) return
    const bar = call.closest<HTMLElement>('[data-chrome="bar"]')
    if (!bar) return
    const run = (): void => {
      // No layout (a test DOM): leave the full bar.
      if (bar.clientWidth <= 0) return
      fitCallBar(bar, call)
    }
    run()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(run)
    ro.observe(bar)
    ro.observe(call)
    if (call.parentElement) ro.observe(call.parentElement)
    return () => ro.disconnect()
  }, [on])
  return ref
}
