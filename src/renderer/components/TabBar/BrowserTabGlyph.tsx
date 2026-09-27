import { useEffect, useState } from 'react'
import { paintableBrowserIcon } from '@/lib/browser-tab-icon'

/** Globe used when a browser tab has no paintable favicon yet. */
function BrowserGlobe(): React.JSX.Element {
  return (
    <svg className="w-3 h-3 text-[var(--color-text-muted)] opacity-70" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="8" cy="8" r="6.5" />
      <path d="M1.5 8h13M8 1.5c1.8 1.7 2.8 4 2.8 6.5S9.8 13.3 8 14.5C6.2 12.8 5.2 10.5 5.2 8S6.2 3.2 8 1.5z" />
    </svg>
  )
}

/**
 * Site icon for a browser tab. 16px fits the h-9 strip and the 24px pane
 * row (the file glyph beside them is 16). Square corners. A missing or
 * broken icon stays the globe — never an https URL (shell img-src).
 */
export function BrowserTabGlyph({ icon }: { icon?: string | null }): React.JSX.Element {
  const paintable = paintableBrowserIcon(icon)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    setFailed(false)
  }, [paintable])
  if (!paintable || failed) return <BrowserGlobe />
  return (
    <img
      src={paintable}
      alt=""
      draggable={false}
      className="flex-shrink-0"
      style={{ width: 16, height: 16, borderRadius: 0, objectFit: 'contain' }}
      onError={() => setFailed(true)}
    />
  )
}
