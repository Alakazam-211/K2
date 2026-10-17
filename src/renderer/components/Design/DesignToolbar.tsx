// Design tab controls (DD7, reworked Rosson 2026-10-16).
//
//  - `DesignPaletteButton` sits in the Browser chrome bar after Back /
//    Forward, before the URL. It IS the Select toggle: accent colour while
//    Select is on (the default when the tab opens), the normal icon colour
//    when off. No separate Select button and no S hotkey.
//  - There is no status strip above the page any more: the note count,
//    Send all and Retry live in the notes tray's header (NoteTray).

import { K2_ICON_PX, PaletteIcon } from '@/components/icons/K2Icons'

export interface DesignPaletteButtonProps {
  /** Select (pick an element) is on. */
  armed: boolean
  onToggle: () => void
  /** Select cannot run here (web client, remote loopback refused…). */
  disabled?: boolean
}

/** The palette in the chrome bar: the Select toggle. */
export function DesignPaletteButton({ armed, onToggle, disabled = false }: DesignPaletteButtonProps): React.JSX.Element {
  return (
    <button
      type="button"
      aria-pressed={armed}
      aria-label="Select an element"
      title={armed ? 'Selecting: click an element on the page (Esc stops)' : 'Select an element on the page'}
      disabled={disabled}
      onClick={onToggle}
      data-design-palette={armed ? 'on' : 'off'}
      className={`ml-1 flex h-6 w-6 flex-shrink-0 items-center justify-center cursor-pointer disabled:opacity-30 ${
        armed
          ? 'text-[var(--color-accent)] hover:text-[var(--color-accent-hover,var(--color-accent))]'
          : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text)]'
      }`}
    >
      <PaletteIcon size={K2_ICON_PX} />
    </button>
  )
}
