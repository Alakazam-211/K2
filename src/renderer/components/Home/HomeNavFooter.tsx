// The Home nav's bottom row (K2 Design's Home nav mock):
// [ ⌘K search field ][ + ].
//
// - The field is the command palette's entry point, not a filter: it reads
//   like the palette ("Search workspaces…", with a ⌘K hint) and clicking it,
//   typing in it (the text carries over) or ⌘K opens the real palette.
//   When the palette closes with nothing focused, focus comes back here.
// - + opens Add to Home on the current group (`AddToHomeModal`).
//
// The row is a container: under 12.5rem (a nav under about 225px) the ⌘K
// hint drops, so the placeholder keeps its room. The 8px gap is the
// footer's (`NAV_COLLAPSE_GAP_PX`).

import { forwardRef, useRef } from 'react'
import { Button, cx } from '@/components/ui'
import { SearchField } from '@/components/ui/SearchField'
import { NAV_FOOTER_ROW_CLASS, NavFooterPlus } from '@/components/Sidebar/NavFooterButton'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { useHomeAddPickerStore } from './home-room'

/** The palette's own wording (CommandPalette.tsx: "Search workspaces..."). */
export const NAV_PALETTE_PLACEHOLDER = 'Search workspaces…'
export const NAV_PALETTE_LABEL = 'Search workspaces (⌘K)'

/** The footer field's height: the footer row's (32px), not the standalone 36px. */
const FIELD_STYLE = { '--control-search-height': '32px', paddingRight: 40 } as React.CSSProperties

/** Open the palette from `field`, with `text`; when it closes and nothing
 *  else took focus, focus comes back to `field`. */
function openPaletteFrom(field: HTMLElement | null, text: string): void {
  useCommandPaletteStore.getState().open(text)
  const off = useCommandPaletteStore.subscribe((s) => {
    if (s.isOpen) return
    off()
    requestAnimationFrame(() => {
      const a = document.activeElement
      if (field && field.isConnected && (!a || a === document.body)) field.focus()
    })
  })
}

/** The ⌘K field: looks like a search field, opens the palette. */
export function NavPaletteField({ className }: { className?: string }): React.JSX.Element {
  const ref = useRef<HTMLInputElement>(null)
  return (
    <span className={cx('relative flex min-w-0 flex-1 items-center', className)} data-home-palette-field="">
      <SearchField
        ref={ref}
        type="text"
        value=""
        placeholder={NAV_PALETTE_PLACEHOLDER}
        aria-label={NAV_PALETTE_LABEL}
        aria-haspopup="dialog"
        aria-keyshortcuts="Meta+K"
        title={NAV_PALETTE_LABEL}
        autoComplete="off"
        spellCheck={false}
        escapeClears={false}
        className="w-full"
        style={FIELD_STYLE}
        data-home-palette-input=""
        onChange={(e) => {
          const text = e.target.value
          if (text) openPaletteFrom(ref.current, text)
        }}
        onClick={() => openPaletteFrom(ref.current, '')}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === 'ArrowDown') {
            e.preventDefault()
            openPaletteFrom(ref.current, '')
          }
        }}
      />
      <kbd
        aria-hidden="true"
        data-home-palette-kbd=""
        className="pointer-events-none absolute right-2 z-[2] border border-[var(--color-border)] bg-[var(--color-bg)] px-1.5 py-0.5 text-[10px] leading-none text-[var(--color-text-muted)] rounded-[var(--radius-selector)] @max-[12.5rem]:hidden"
      >
        ⌘K
      </kbd>
    </span>
  )
}

/** + : Add to Home on the current group. `place` says which + opened it
 *  (focus goes back to it on close). The footer's quiet control: the
 *  control edge, cut corners in Terminal, round in Glass, 32 × 32. */
export const HomeAddButton = forwardRef<
  HTMLButtonElement,
  { place: 'footer' | 'empty'; label?: string; title?: string; className?: string }
>(function HomeAddButton({ place, label = 'Add to Home', title = 'Add to Home', className }, ref) {
  return (
    <Button
      ref={ref}
      variant="quiet"
      size="md"
      data-frost-shape="round"
      data-nav-control=""
      data-home-add-open={place}
      aria-label={label}
      title={title}
      aria-haspopup="dialog"
      className={cx('flex h-8 w-8 flex-shrink-0 items-center justify-center p-0!', className)}
      onClick={(e) => useHomeAddPickerStore.getState().setOpen(true, e.currentTarget)}
    >
      <NavFooterPlus />
    </Button>
  )
})

/** The bottom row: the ⌘K field, then +. */
export function HomeNavFooterRow(): React.JSX.Element {
  return (
    <div className={`${NAV_FOOTER_ROW_CLASS} items-center gap-2`} data-home-footer-row="">
      <NavPaletteField />
      <HomeAddButton place="footer" />
    </div>
  )
}
