// The tab strip's pin (Rosson 2026-10-16): any tab can be pinned from the
// tab itself. An unpinned tab shows a pin next to its × on hover; a pinned
// tab shows the pin where the × was (no ×, like a browser's pinned tab) and
// hovering it turns it into Unpin. Closing a pinned tab is the right-click
// menu's Close Tab. See `lib/pinned-tabs.ts` for ordering.

import { PinIcon, PinOffIcon } from '@/components/icons/K2Icons'

const PIN_PX = 12

/** Hover pin on an unpinned tab. Its slot is always there (invisible until
 *  the tab is hovered) so hovering never widens the tab. */
export function TabPinButton({ onPin }: { onPin: (e: React.MouseEvent) => void }): React.JSX.Element {
  return (
    <button
      type="button"
      data-tab-pin=""
      aria-label="Pin tab"
      title="Pin tab (keeps it at the left)"
      className="-mr-1 ml-2 flex h-4 w-4 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] opacity-0 transition-opacity hover:bg-[var(--color-wash-2)] hover:text-[var(--color-text-primary)] focus-visible:opacity-100 group-hover:opacity-100"
      onClick={onPin}
    >
      <PinIcon size={PIN_PX} />
    </button>
  )
}

/** A pinned tab's close-slot glyph: the pin (accent) at rest, Unpin while
 *  the slot is hovered. Lives in the close button (`group/close`), so the
 *  slot's working/attention marks still take it the same way. */
export function PinnedTabSlotGlyph(): React.JSX.Element {
  return (
    <>
      <span data-tab-pinned-glyph="" className="block text-[var(--color-accent)] group-hover/close:hidden">
        <PinIcon size={PIN_PX} />
      </span>
      <span data-tab-unpin-glyph="" className="hidden group-hover/close:block">
        <PinOffIcon size={PIN_PX} />
      </span>
    </>
  )
}
