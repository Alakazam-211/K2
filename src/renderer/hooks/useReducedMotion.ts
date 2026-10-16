import { useEffect, useState } from 'react'
import { REDUCED_MOTION_QUERY, zenMediaMatches } from '@/lib/zen/zen-theme'

/** `prefers-reduced-motion`, live (re-renders when the OS setting flips). */
export function useReducedMotion(): boolean {
  const [on, setOn] = useState(() => zenMediaMatches(REDUCED_MOTION_QUERY))
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(REDUCED_MOTION_QUERY)
    const sync = (): void => setOn(mq.matches)
    sync()
    mq.addEventListener?.('change', sync)
    return () => mq.removeEventListener?.('change', sync)
  }, [])
  return on
}
