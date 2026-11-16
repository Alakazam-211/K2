// One section of the scene's side panel, as the mock draws it: a head that
// folds it (a chevron, the name in small capitals, a quiet count at the
// right), then its body. The order is the PANEL array (lib/live-view/scene/
// panel.ts); whether it starts open is the panel's (narrow: People and
// Properties closed).

import { ChevronGlyph } from '../shell/glyphs'

export function SceneSection({
  id,
  title,
  count,
  open,
  onToggle,
  flush = false,
  grow = false,
  children,
}: {
  id: string
  title: string
  count?: string | null
  open: boolean
  onToggle: () => void
  /** The body runs edge to edge (the outliner's rows). */
  flush?: boolean
  grow?: boolean
  children: React.ReactNode
}): React.JSX.Element {
  const bodyId = `lv-scene-body-${id}`
  return (
    <section data-testid={`lv-scene-section-${id}`} className="lv-ssec" data-grow={grow ? '' : undefined} aria-label={title}>
      <h3 className="m-0">
        <button type="button" data-k2-focus="" data-testid={`lv-scene-head-${id}`} aria-expanded={open} aria-controls={bodyId} onClick={onToggle} className="lv-shead no-drag">
          <span aria-hidden="true" className="lv-chev">
            <ChevronGlyph size={11} />
          </span>
          <span>{title}</span>
          {count ? (
            <span data-testid="lv-scene-count" className="lv-scount">
              {count}
            </span>
          ) : null}
        </button>
      </h3>
      {open ? (
        <div id={bodyId} className="lv-sbody" data-flush={flush ? '' : undefined}>
          {children}
        </div>
      ) : null}
    </section>
  )
}
