// @vitest-environment jsdom
//
// The file tab's conflict notice: `doc_conflict_copy` for this file shows the
// daemon's text once (a repeat of the same copy doesn't stack), another file's
// event shows nothing, and Dismiss removes it.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { DocConflictCopyEvent } from '@/stores/session-events'
import type { ServerScope } from '@/kessel/server-scope'

const bus = vi.hoisted(() => ({
  handlers: new Set<(e: DocConflictCopyEvent) => void>(),
}))

vi.mock('@/stores/session-events', () => ({
  onDocConflictCopy: (_scope: unknown, fn: (e: DocConflictCopyEvent) => void) => {
    bus.handlers.add(fn)
    return () => bus.handlers.delete(fn)
  },
}))

import { DocConflictNotice, conflictFilePath } from './DocConflictNotice'

const scope = { id: 'local', connectionKey: 'local', viewOnly: false } as unknown as ServerScope
const TEXT = "Someone saved over Ada's change to notes.md. Both are kept: .k2/conflicts/notes (Ada's version).md"

function ev(over: Partial<DocConflictCopyEvent> = {}): DocConflictCopyEvent {
  return {
    kind: 'doc_conflict_copy',
    projectId: 'p1',
    workspace: '/w/proj/',
    rel: 'notes.md',
    current: 4,
    kept: 3,
    copy: ".k2/conflicts/notes (Ada's version).md",
    text: TEXT,
    ...over,
  }
}

function send(e: DocConflictCopyEvent): void {
  act(() => {
    for (const fn of bus.handlers) fn(e)
  })
}

afterEach(() => {
  cleanup()
  bus.handlers.clear()
})

describe('DocConflictNotice', () => {
  it('joins the workspace and rel into the file path', () => {
    expect(conflictFilePath({ workspace: '/w/proj/', rel: 'a/b.png' })).toBe('/w/proj/a/b.png')
    expect(conflictFilePath({ workspace: '/w/proj', rel: 'notes.md' })).toBe('/w/proj/notes.md')
  })

  it('shows the daemon text once for this file', () => {
    render(<DocConflictNotice scope={scope} filePath="/w/proj/notes.md" />)
    expect(screen.queryByTestId('doc-conflict-notice')).toBeNull()
    send(ev())
    send(ev())
    const shown = screen.getAllByTestId('doc-conflict-notice')
    expect(shown).toHaveLength(1)
    expect(shown[0].textContent).toContain(TEXT)
  })

  it("ignores another file's conflict", () => {
    render(<DocConflictNotice scope={scope} filePath="/w/proj/notes.md" />)
    send(ev({ rel: 'other.md' }))
    send(ev({ workspace: '/w/elsewhere' }))
    expect(screen.queryByTestId('doc-conflict-notice')).toBeNull()
  })

  it('a second copy is a second notice, and Dismiss removes one', () => {
    render(<DocConflictNotice scope={scope} filePath="/w/proj/notes.md" />)
    send(ev())
    send(ev({ current: 6, kept: 5, text: 'second' }))
    expect(screen.getAllByTestId('doc-conflict-notice')).toHaveLength(2)
    fireEvent.click(screen.getAllByText('Dismiss')[0])
    const left = screen.getAllByTestId('doc-conflict-notice')
    expect(left).toHaveLength(1)
    expect(left[0].textContent).toContain('second')
  })
})
