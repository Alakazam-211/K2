import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  ELLIPSIS,
  middleEllipsis,
  resourceNameSlotWidth,
  treeNameSlotWidth,
} from './middleEllipsis'

// Review glyph width at 13px Meslo, including U+2026. Test-only — the
// panel measures a live glyph and must not hard-code this.
const GLYPH = 7.83
const measure = (text: string): number => text.length * GLYPH

const MEETING = 'notes_from_the_monday_meeting_v2.md'
const PREFERRED_TAIL = 'y_meeting_v2.md'

function ellipsisCount(text: string): number {
  return text.split(ELLIPSIS).length - 1
}

function assertKeepsEnd(name: string, shown: string): void {
  expect(ellipsisCount(shown)).toBe(1)
  expect(shown.includes('...')).toBe(false)
  const head = shown.slice(0, shown.indexOf(ELLIPSIS))
  const tail = shown.slice(shown.indexOf(ELLIPSIS) + 1)
  expect(name.startsWith(head)).toBe(true)
  expect(name.endsWith(tail)).toBe(true)
  expect(head.length + tail.length).toBeLessThan(name.length)
}

describe('middleEllipsis', () => {
  it('keeps a 12-character stem plus the extension when that tail fits', () => {
    expect(MEETING.endsWith(PREFERRED_TAIL)).toBe(true)
    expect(PREFERRED_TAIL.slice(0, -'.md'.length)).toHaveLength(12)

    const tailBox = measure(ELLIPSIS + PREFERRED_TAIL)
    expect(tailBox).toBeCloseTo(125.28, 5)
    expect(measure(MEETING)).toBeGreaterThan(tailBox)

    const atTail = middleEllipsis(MEETING, tailBox, measure)
    assertKeepsEnd(MEETING, atTail)
    expect(atTail.endsWith(PREFERRED_TAIL)).toBe(true)
    expect(atTail.slice(atTail.indexOf(ELLIPSIS) + 1)).toBe(PREFERRED_TAIL)

    // Wider than the tail, still short of the whole name: extra width is head, not a longer tail.
    const wider = tailBox + 40
    expect(wider).toBeLessThan(measure(MEETING))
    const grown = middleEllipsis(MEETING, wider, measure)
    assertKeepsEnd(MEETING, grown)
    expect(grown.endsWith(PREFERRED_TAIL)).toBe(true)
    expect(grown.slice(grown.indexOf(ELLIPSIS) + 1)).toBe(PREFERRED_TAIL)
    expect(grown.slice(0, grown.indexOf(ELLIPSIS)).length).toBeGreaterThan(0)
  })

  it('shrinks the tail but still ends with the extension when the 12-character tail does not fit', () => {
    const tailBox = measure(ELLIPSIS + PREFERRED_TAIL)
    const narrower = tailBox - 0.5
    expect(measure(ELLIPSIS + '.md')).toBeLessThan(narrower)

    const shown = middleEllipsis(MEETING, narrower, measure)
    assertKeepsEnd(MEETING, shown)
    expect(shown.endsWith('.md')).toBe(true)
    const tail = shown.slice(shown.indexOf(ELLIPSIS) + 1)
    expect(tail.length).toBeLessThan(PREFERRED_TAIL.length)
    expect(tail.slice(0, -'.md'.length).length).toBeLessThan(12)
    expect(shown.length).toBeLessThan((ELLIPSIS + PREFERRED_TAIL).length)
  })

  it('cuts into the extension when the extension itself does not fit', () => {
    const width = measure(ELLIPSIS + 'md')
    expect(measure(ELLIPSIS + '.md')).toBeGreaterThan(width)
    const shown = middleEllipsis(MEETING, width, measure)
    assertKeepsEnd(MEETING, shown)
    expect(shown.endsWith('.md')).toBe(false)
    expect(MEETING.endsWith(shown.slice(shown.indexOf(ELLIPSIS) + 1))).toBe(true)
  })

  it('returns the whole string and no ellipsis when the box is wide enough', () => {
    expect(middleEllipsis(MEETING, measure(MEETING), measure)).toBe(MEETING)
    expect(middleEllipsis(MEETING, measure(MEETING) + 80, measure)).toBe(MEETING)
    expect(middleEllipsis(MEETING, measure(MEETING) + 80, measure).includes(ELLIPSIS)).toBe(false)

    const short = 'short_name.md'
    expect(measure(short)).toBeLessThan(measure(ELLIPSIS + short))
    expect(middleEllipsis(short, measure(short), measure)).toBe(short)
    expect(middleEllipsis('a.md', 1000, measure)).toBe('a.md')
  })

  it('returns a.md unchanged at real drawer widths', () => {
    for (const drawer of [180, 240, 400]) {
      for (let depth = 0; depth <= 4; depth++) {
        const slot = treeNameSlotWidth(drawer, depth)
        expect(slot).toBeGreaterThanOrEqual(measure('a.md'))
        expect(middleEllipsis('a.md', slot, measure)).toBe('a.md')
      }
    }
  })

  it('widening the box restores more of the head until the whole name fits', () => {
    const tailBox = measure(ELLIPSIS + PREFERRED_TAIL)
    const widths = [tailBox, tailBox + 30, tailBox + 70, measure(MEETING) - 8, measure(MEETING)]
    let prevHead = -1
    for (const width of widths) {
      const shown = middleEllipsis(MEETING, width, measure)
      if (width >= measure(MEETING)) {
        expect(shown).toBe(MEETING)
        expect(shown.includes(ELLIPSIS)).toBe(false)
        continue
      }
      assertKeepsEnd(MEETING, shown)
      expect(shown.endsWith(PREFERRED_TAIL)).toBe(true)
      const headLen = shown.indexOf(ELLIPSIS)
      expect(headLen).toBeGreaterThan(prevHead)
      prevHead = headLen
    }
  })

  it('does not invent an extension for a leading-dot name or a directory', () => {
    expect(middleEllipsis('.env', treeNameSlotWidth(180, 0), measure)).toBe('.env')
    expect(middleEllipsis('.gitignore', treeNameSlotWidth(240, 2), measure)).toBe('.gitignore')

    const hidden = '.environment_production_credentials_file'
    expect(hidden.indexOf('.')).toBe(0)
    expect(hidden.lastIndexOf('.')).toBe(0)
    const tail = hidden.slice(-12)
    const head = hidden.slice(0, 3)
    const width = measure(head + ELLIPSIS + tail)
    expect(measure(hidden)).toBeGreaterThan(width)
    const shown = middleEllipsis(hidden, width, measure)
    expect(shown).toBe(head + ELLIPSIS + tail)
    expect(shown.endsWith(tail)).toBe(true)

    const dir = 'notes_from_the_monday_meeting.backup'
    const ext = '.backup'
    const stem = dir.slice(0, dir.lastIndexOf('.'))
    const fileTail = stem.slice(-12) + ext
    expect(fileTail.endsWith(ext)).toBe(true)
    const dirTail = dir.slice(-12)
    expect(dirTail).not.toBe(fileTail)

    const asFile = middleEllipsis(dir, measure(ELLIPSIS + fileTail), measure)
    expect(asFile.endsWith(fileTail)).toBe(true)
    expect(asFile.endsWith(ext)).toBe(true)

    const asDir = middleEllipsis(dir, measure(ELLIPSIS + dirTail), measure, { directory: true })
    expect(asDir.endsWith(dirTail)).toBe(true)
    expect(asDir.endsWith(fileTail)).toBe(false)

    const makefile = 'Makefile_from_the_vendor_tree'
    const makeTail = makefile.slice(-12)
    expect(middleEllipsis(makefile, measure(ELLIPSIS + makeTail), measure).endsWith(makeTail)).toBe(true)
    expect(middleEllipsis(makefile, measure(ELLIPSIS + makeTail), measure).includes('.')).toBe(false)
  })

  it('keeps only the last extension of archive.tar.gz', () => {
    const name = 'abcdefghijklmnopqrstuvwxyz.tar.gz'
    const stem = name.slice(0, name.lastIndexOf('.'))
    expect(stem.endsWith('.tar')).toBe(true)
    const fileTail = stem.slice(-12) + '.gz'
    const wrongExt = name.slice(0, name.lastIndexOf('.tar.gz')).slice(-12) + '.tar.gz'
    expect(fileTail.endsWith('.gz')).toBe(true)
    expect(fileTail).not.toBe(wrongExt)

    const shown = middleEllipsis(name, measure(ELLIPSIS + fileTail), measure)
    assertKeepsEnd(name, shown)
    expect(shown.endsWith(fileTail)).toBe(true)
    expect(shown.endsWith('.gz')).toBe(true)
    expect(shown.includes(wrongExt)).toBe(false)

    const narrow = measure(ELLIPSIS + 'z.gz')
    expect(measure(ELLIPSIS + '.tar.gz')).toBeGreaterThan(narrow)
    expect(measure(ELLIPSIS + '.gz')).toBeLessThanOrEqual(narrow)
    const shrunk = middleEllipsis(name, narrow, measure)
    assertKeepsEnd(name, shrunk)
    expect(shrunk.endsWith('.gz')).toBe(true)
    expect(shrunk.slice(shrunk.indexOf(ELLIPSIS) + 1, -'.gz'.length).length).toBeLessThan(12)

    expect(middleEllipsis('archive.tar.gz', measure('archive.tar.gz'), measure)).toBe('archive.tar.gz')
  })

  it('shrinks in drawer slots that are narrower than the 12-character tail', () => {
    expect(treeNameSlotWidth(180, 0)).toBe(128)
    expect(treeNameSlotWidth(180, 1)).toBe(112)
    expect(treeNameSlotWidth(240, 4)).toBe(124)

    const tailBox = measure(ELLIPSIS + PREFERRED_TAIL)
    const depth0 = treeNameSlotWidth(180, 0)
    expect(depth0).toBeGreaterThanOrEqual(tailBox)
    expect(middleEllipsis(MEETING, depth0, measure).endsWith(PREFERRED_TAIL)).toBe(true)

    for (const slot of [treeNameSlotWidth(180, 1), treeNameSlotWidth(240, 4)]) {
      expect(slot).toBeLessThan(tailBox)
      expect(measure(ELLIPSIS + '.md')).toBeLessThanOrEqual(slot)
      const shown = middleEllipsis(MEETING, slot, measure)
      assertKeepsEnd(MEETING, shown)
      expect(shown.endsWith('.md')).toBe(true)
      const tail = shown.slice(shown.indexOf(ELLIPSIS) + 1)
      expect(tail.length).toBeLessThan(PREFERRED_TAIL.length)
      expect(tail.slice(0, -'.md'.length).length).toBeLessThan(12)
    }
  })

  it('clamps a non-positive slot to empty when the name does not fit', () => {
    expect(treeNameSlotWidth(0, 4)).toBe(0)
    expect(middleEllipsis('a.md', 0, measure)).toBe('')
    expect(resourceNameSlotWidth(10)).toBe(0)
    const plain = resourceNameSlotWidth(240)
    const missing = resourceNameSlotWidth(240, { missing: true, badgeWidthPx: 40 })
    expect(missing).toBe(plain - 6 - 40)
    expect(missing).toBeGreaterThan(0)
  })
})

describe('FileTree call sites', () => {
  const src = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), 'FileTree.tsx'),
    'utf8',
  )

  it('middle-ellipsizes only the tree name and the resource basename', () => {
    const uses = src.match(/<MiddleEllipsisName/g)
    expect(uses).toHaveLength(2)
    expect(src).toContain('title={entry.name}')
    expect(src).toContain('title={row.filePath}')
    expect(src).not.toContain('title={row.fileName}')
    expect(src).not.toMatch(/MiddleEllipsisName[\s\S]{0,500}title=\{row\.fileName\}/)
    expect(src).toContain('data-path={entry.path}')
    expect(src).toContain('directory={entry.isDirectory}')

    // Left alone: env chips, AI config chips, load-error floater, create toast.
    expect(src).toContain(
      '<span className="truncate">{entry.path.startsWith(rootPath) ? entry.path.slice(rootPath.length + 1) : entry.name}</span>',
    )
    expect(src).toContain('<span className="truncate">{entry.name}</span>')
    expect(src).toContain('className="text-[var(--color-status-error-soft)] truncate" title={dirPath}')
    expect(src).toContain('toast.addToast(`Create failed: ${err}`, \'error\')')
    expect(src).toContain('title={entry.path}')
  })

  it('does not feed the ellipsis display string to rename, copy, or open', () => {
    expect(src).toContain('initialValue={entry.name}')
    expect(src).toContain('navigator.clipboard.writeText(entry.path)')
    expect(src).toContain('openFileAsTab(entry.path)')
    expect(src).toContain('openFileAsTab(row.filePath)')
    expect(src).toContain('useFileSelectionStore.getState().select(entry.path)')
    expect(src).toContain('useFileSelectionStore.getState().select(row.filePath)')

    expect(src).not.toMatch(/initialValue=\{[^}]*middleEllipsis/)
    expect(src).not.toMatch(/initialValue=\{[^}]*MiddleEllipsis/)
    expect(src).not.toMatch(/writeText\(\s*middleEllipsis/)
    expect(src).not.toMatch(/openFileAsTab\(\s*middleEllipsis/)
    expect(src).not.toMatch(/openFileAsTab\(\s*row\.fileName/)
    expect(src).not.toContain('initialValue={shown}')
    expect(src).not.toContain('middleEllipsis(')
  })
})
