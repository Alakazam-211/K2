// @vitest-environment jsdom
// workbench-v1 B7: K2's changed-files card per turn, on every Thread surface
// (the desktop Thread row, the shared bubble that Zen's Conversation uses)
// and in a Garden widget's closed Thread item.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { ThreadItemRow } from './ThreadOverlayPane'
import { ThreadBubble, threadBubbleText } from '@/components/Thread/ThreadZenKit'
import { ChangesCard, changeLetter, changesHeadline } from './ChangesCard'
import { coerceChanges, threadItemsFromSnapshot, type OverlayThreadItem } from './overlayThread'
import { projectZenThreadItem } from '@/lib/zen/zen-custom-projection'

afterEach(() => cleanup())

const CHANGES = {
  files: [
    { file: 'src/login.ts', oldFile: null, status: 'modified', additions: 30, deletions: 10 },
    { file: 'src/new.rs', oldFile: null, status: 'added', additions: 9, deletions: 0 },
    { file: 'old.txt', oldFile: null, status: 'deleted', additions: 0, deletions: 2 },
    { file: 'logo.png', oldFile: null, status: 'modified', additions: null, deletions: null },
  ],
  total: 6,
  additions: 41,
  deletions: 12,
  commits: 2,
  from: 'aaaaaaa',
  to: 'bbbbbbb',
  branch: 'k2/fix-login',
  truncated: true,
}

const DOC = {
  id: 'chg1',
  kind: 'changes',
  from: 'k2',
  via: 'k2',
  body: 'Changed 6 files (+41 −12) · 2 commits on k2/fix-login',
  created_at: 1761062000,
  changes: CHANGES,
}

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 4, id: doc.id, doc }] }).items[0]
}

describe('coerceChanges', () => {
  it('reads the closed shape and drops what it does not know', () => {
    const c = coerceChanges({ ...CHANGES, extra: 'x', files: [...CHANGES.files, { file: 5 }, { file: 'a', status: 'added', path: '/abs/a' }] })
    expect(c).not.toBeNull()
    expect(c!.files.map((f) => f.file)).toEqual(['src/login.ts', 'src/new.rs', 'old.txt', 'logo.png', 'a'])
    expect(Object.keys(c!.files[4]).sort()).toEqual(['additions', 'deletions', 'file', 'oldFile', 'status'])
    expect((c as unknown as Record<string, unknown>).extra).toBeUndefined()
    expect(c!.truncated).toBe(true)
  })

  it('is null for anything that is not a changes object', () => {
    expect(coerceChanges(null)).toBeNull()
    expect(coerceChanges('x')).toBeNull()
    expect(coerceChanges({ total: 3 })).toBeNull()
    expect(coerceChanges([])).toBeNull()
  })

  it('a snapshot row keeps the doc.changes', () => {
    const item = itemOf(DOC)
    expect(item.doc.kind).toBe('changes')
    expect(item.doc.changes?.files).toHaveLength(4)
    expect(item.doc.changes?.branch).toBe('k2/fix-login')
  })
})

describe('the card', () => {
  it('headline, letters, +/− and the rest', () => {
    expect(changesHeadline(coerceChanges(CHANGES)!)).toBe('6 files changed · +41 −12 · 2 commits')
    expect(changesHeadline({ ...coerceChanges(CHANGES)!, total: 0, additions: 0, deletions: 0, commits: 1 })).toBe('1 commit')
    expect([changeLetter('added'), changeLetter('untracked'), changeLetter('deleted'), changeLetter('renamed'), changeLetter('modified')]).toEqual([
      'A',
      'A',
      'D',
      'R',
      'M',
    ])
    render(<ChangesCard changes={coerceChanges(CHANGES)!} />)
    const card = screen.getByTestId('changes-card')
    expect(card.querySelectorAll('[data-changes-file]')).toHaveLength(4)
    expect(card.querySelector('[data-changes-file="src/login.ts"]')?.textContent).toContain('+30')
    expect(card.querySelector('[data-changes-file="logo.png"]')?.textContent).toBe('Mlogo.png')
    expect(card.querySelector('[data-changes-more]')?.textContent).toBe('and 2 more files')
    expect(card.querySelector('[data-changes-branch]')?.textContent).toBe('on k2/fix-login · aaaaaaa..bbbbbbb')
  })

  it('draws in the shared bubble and the desktop Thread row, not as its plain body', () => {
    const item = itemOf(DOC)
    expect(threadBubbleText(item)).toBe('')
    const a = render(<ThreadBubble item={item} mine={false} author="k2" timeLabel="now" viewer={null} nowSec={1761062000} />)
    expect(a.getByTestId('changes-card')).toBeTruthy()
    expect(a.container.textContent).not.toContain('Changed 6 files')
    cleanup()
    const b = render(<ThreadItemRow item={item} nowSec={1761062000} onAnswer={() => undefined} onVoid={() => undefined} />)
    expect(b.getByTestId('changes-card')).toBeTruthy()
  })

  it('an older doc with no changes object shows its body', () => {
    const item = itemOf({ ...DOC, changes: undefined })
    expect(threadBubbleText(item)).toBe(DOC.body)
  })
})

describe('Garden widgets', () => {
  it('get the changes item in the closed ThreadDoc shape', () => {
    const item = itemOf(DOC)
    const p = projectZenThreadItem(item)
    expect(p).not.toBeNull()
    const doc = p!.doc as Record<string, unknown>
    expect(doc.kind).toBe('changes')
    expect(doc.body).toBe(DOC.body)
    expect(doc.changes).toEqual(coerceChanges(CHANGES))
    // A copy, never the doc's own object.
    expect(doc.changes).not.toBe(item.doc.changes)
  })

  it('a text item has no changes field', () => {
    const p = projectZenThreadItem(itemOf({ id: 't1', kind: 'text', from: 'a', body: 'hi' }))
    expect(Object.prototype.hasOwnProperty.call(p!.doc, 'changes')).toBe(false)
  })
})
