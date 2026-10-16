import { lazy, Suspense, useCallback, useState } from 'react'
import { HelpIcon, TOP_BAR_ICON_ACTIVE_CLASS, TOP_BAR_ICON_REST_CLASS } from '@/components/icons/K2Icons'

// The explorer and its generated CLI catalog load on first open, not with
// the top bar.
const HelpExplorer = lazy(() => import('./HelpExplorer'))

const noDrag = {
  WebkitAppRegion: 'no-drag',
} as React.CSSProperties

/** Top-bar Help: opens the CLI tool explorer (Rosson 2026-10-16; was the
 *  K2 cheat sheet). Rests like the page tabs, brighter while open. */
export default function HelpButton(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const close = useCallback(() => setOpen(false), [])

  return (
    <>
      <button
        type="button"
        data-testid="help-button"
        onClick={() => setOpen(true)}
        aria-pressed={open}
        aria-haspopup="dialog"
        className={`flex h-6 w-6 items-center justify-center ${open ? TOP_BAR_ICON_ACTIVE_CLASS : TOP_BAR_ICON_REST_CLASS} transition-colors no-drag`}
        style={noDrag}
        title="Help"
        aria-label="Help"
      >
        {/* Icon style A3: Lucide `circle-help`, squared. */}
        <HelpIcon />
      </button>
      {open ? (
        <Suspense fallback={null}>
          <HelpExplorer onClose={close} />
        </Suspense>
      ) : null}
    </>
  )
}
