import { useEffect, useRef } from 'react'

/** The edge of a header above a scrolling list: four stacked masked blur
 *  layers under a dark tint, the phone's header stack. Place it at the top of
 *  a `relative` box holding the list. Only Glass draws it (globals.css,
 *  .k2-header-blur), and only once the list has scrolled, so the first row is
 *  never dimmed at rest. Live blur over a scrolling list is expensive: at most
 *  two on screen. `early` and `heavy` are the phone's variants. */
export function ProgressiveHeaderBlur({ variant }: { variant?: 'early' | 'heavy' }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = ref.current
    const host = el?.parentElement
    if (!el || !host) return
    const onScroll = (e: Event): void => {
      const t = e.target
      // Not every scroll is the list: a wide code block scrolls sideways.
      if (!(t instanceof HTMLElement) || t.scrollHeight <= t.clientHeight) return
      el.dataset.scrolled = t.scrollTop > 0 ? '1' : '0'
    }
    host.addEventListener('scroll', onScroll, { capture: true, passive: true })
    return () => host.removeEventListener('scroll', onScroll, { capture: true })
  }, [])
  return (
    <div ref={ref} aria-hidden="true" className={variant ? `k2-header-blur k2-header-blur--${variant}` : 'k2-header-blur'}>
      <span className="k2-header-blur-1" />
      <span className="k2-header-blur-2" />
      <span className="k2-header-blur-3" />
      <span className="k2-header-blur-4" />
      <span className="k2-header-tint" />
    </div>
  )
}
