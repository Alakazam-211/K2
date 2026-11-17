// The chat's model, named in the composer's own action row: quiet text
// ("Opus 5.5 ⌄"), no border, left of the stash button. Like the stash
// button it shows only while the box is empty (no text, no files); the
// caller decides that. A click opens the model picker (the typed `/model`
// opens the same one, shown or not). On a narrow composer it names the
// family alone ("Opus ⌄").
//
// `zen`: the Thread's box (its muted ink, the soft hover fill).
// `square`: Terminal view's bar (the app's muted text, square corners).

import { useLayoutEffect, useRef, useState, type JSX } from 'react'
import { modelFamilyLabel, modelLabel } from '@/lib/model-choices'

/** Under this composer width (px) the control names the family alone. */
export const COMPOSE_MODEL_SHORT_BELOW_PX = 560

/** The control's words for a composer `width` px wide (0: not laid out). */
export function composeModelLabel(id: string, width: number): string {
  return width > 0 && width < COMPOSE_MODEL_SHORT_BELOW_PX ? modelFamilyLabel(id) : modelLabel(id)
}

/** The "opens a menu" mark: a small down chevron in the text's ink. */
function Chevron(): JSX.Element {
  return (
    <svg width={10} height={10} viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden style={{ flex: 'none' }}>
      <path d="M3 4.5 6 7.5 9 4.5" />
    </svg>
  )
}

export function ComposeModelButton({
  model,
  onOpen,
  look,
  height,
  marginBottom = 0,
}: {
  model: string
  onOpen: (anchor: HTMLElement) => void
  look: 'zen' | 'square'
  /** The row's control height (px): Send's. */
  height: number
  marginBottom?: number
}): JSX.Element {
  const ref = useRef<HTMLButtonElement>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const bar = ref.current?.closest<HTMLElement>('[data-compose-bar]')
    if (!bar) return
    const read = (): void => setWidth(bar.clientWidth)
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(bar)
    return () => ro.disconnect()
  }, [])
  const name = modelLabel(model)
  return (
    <button
      ref={ref}
      type="button"
      data-testid="compose-model"
      data-compose-model={look}
      title="Model for this chat (/model)"
      aria-label={`Model for this chat: ${name}. Change`}
      aria-haspopup="listbox"
      // Keep the caret in the box.
      onMouseDown={(e) => e.preventDefault()}
      onClick={(e) => onOpen(e.currentTarget)}
      className="k2-focus inline-flex flex-none items-center whitespace-nowrap cursor-pointer"
      style={{
        gap: 5,
        height,
        marginBottom,
        padding: '0 6px',
        border: 0,
        background: 'transparent',
        font: 'inherit',
        fontSize: look === 'zen' ? 13 : 'var(--type-sm)',
        color: look === 'zen' ? 'var(--zen-text-muted)' : 'var(--color-text-muted)',
      }}
    >
      {composeModelLabel(model, width)}
      <Chevron />
    </button>
  )
}
