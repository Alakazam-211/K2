// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MiddleEllipsisName } from './MiddleEllipsisName'
import { ELLIPSIS, middleEllipsis } from './middleEllipsis'

// Review glyph width at 13px Meslo. Test-only.
const GLYPH = 7.83
const measure = (text: string): number => text.length * GLYPH
const MEETING = 'notes_from_the_monday_meeting_v2.md'
const PREFERRED_TAIL = 'y_meeting_v2.md'

describe('MiddleEllipsisName', () => {
  // FileTree wires this component in two places only:
  // - TreeItem name (search hits use the same row): title={entry.name}.
  //   data-path={entry.path} stays on the row button.
  // - Workspace resource basename: no title here. The resource button
  //   keeps title={row.filePath}.
  // Those call sites are asserted from FileTree source in middleEllipsis.test.ts.
  it('titles the tree name with the full name and omits a title when not asked', () => {
    const width = measure(ELLIPSIS + PREFERRED_TAIL)
    const { rerender } = render(
      <MiddleEllipsisName
        name={MEETING}
        maxWidthPx={width}
        measure={measure}
        title={MEETING}
      />,
    )
    const titled = screen.getByTitle(MEETING)
    expect(titled.textContent).toBe(middleEllipsis(MEETING, width, measure))
    expect(titled.textContent).not.toBe(MEETING)
    expect(titled.textContent).toContain(ELLIPSIS)
    expect(titled.className).not.toMatch(/\btruncate\b/)
    expect(titled.className).toContain('min-w-0')
    expect(titled.className).toContain('flex-1')
    expect(titled.className).toContain('overflow-hidden')
    expect(titled.className).toContain('whitespace-nowrap')

    rerender(
      <MiddleEllipsisName
        name={MEETING}
        maxWidthPx={width}
        measure={measure}
      />,
    )
    expect(document.querySelector('[title]')).toBeNull()
    expect(document.body.textContent).toContain(ELLIPSIS)
  })
})
