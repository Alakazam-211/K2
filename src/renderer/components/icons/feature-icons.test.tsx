import { describe, expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { FEATURE_ICONS, type FeatureIconId } from './feature-icons'
import { SetiFileIcon } from '@/lib/seti-file-icons'

/** An icon's drawing: its markup without the attributes that name or size it. */
function drawing(id: FeatureIconId): string {
  const Icon = FEATURE_ICONS[id] as (p: { size?: number }) => React.JSX.Element
  return renderToStaticMarkup(<Icon size={16} />)
    .replace(/\s(data-[\w-]+|class|width|height|aria-hidden)="[^"]*"/g, '')
}

describe('FEATURE_ICONS', () => {
  const ids = Object.keys(FEATURE_ICONS) as FeatureIconId[]

  it('gives Board its own icon, not Design\'s (Rosson 2026-11-09)', () => {
    expect(FEATURE_ICONS.board).not.toBe(FEATURE_ICONS.design)
    expect(drawing('board')).not.toBe(drawing('design'))
  })

  it('no two features share an icon component', () => {
    const seen = new Map<unknown, FeatureIconId>()
    for (const id of ids) {
      const other = seen.get(FEATURE_ICONS[id])
      expect(other, `${id} shares its icon with ${other}`).toBeUndefined()
      seen.set(FEATURE_ICONS[id], id)
    }
  })

  it('no two features share a drawing', () => {
    const seen = new Map<string, FeatureIconId>()
    for (const id of ids) {
      const d = drawing(id)
      const other = seen.get(d)
      expect(other, `${id} draws the same as ${other}`).toBeUndefined()
      seen.set(d, id)
    }
  })

  it('a Board file\'s icon is the Board icon, not a picture', () => {
    const board = renderToStaticMarkup(<SetiFileIcon name="/w/Plan.K5BOARD" scheme="dark" />)
    expect(board).toContain('data-k2-icon="presentation"')
    expect(board).toContain('data-board-file-icon')
    const png = renderToStaticMarkup(<SetiFileIcon name="logo.png" scheme="dark" />)
    expect(png).not.toContain('data-k2-icon')
  })
})
