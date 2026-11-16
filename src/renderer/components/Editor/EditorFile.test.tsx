// @vitest-environment jsdom
// Saving a plain file in the editor frame (build-editors.md E1c): autosave,
// ⌘S and blur; the expect hash on every write; the failure bars (first and
// second try, Save a copy) with the dot kept; each 409 code; Compare, Undo
// their save and Use their version; drafts and the restore bar; and no
// writes for a read-only file.

import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { useState } from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const post = vi.fn()
const get = vi.fn()
vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliPost: (...a: unknown[]) => post(...a),
  daemonCliGet: (...a: unknown[]) => get(...a),
}))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import { getDraft, putDraft, resetDraftsForTests } from '@/lib/editor/save-model/drafts'
import { sha256Hex } from '@/lib/editor/save-model/write'
import { EditorFile } from './EditorFile'

const scope = { hostKey: 'local', connectionKey: 'local' } as never
const PATH = '/ws/quill/README.md'

function Harness({ initial, baseAbsent = false, readOnly = null, kind = 'markdown', path = PATH }: { initial: string; baseAbsent?: boolean; readOnly?: string | null; kind?: 'markdown' | 'code'; path?: string }): React.JSX.Element {
  const [base, setBase] = useState(initial)
  const [edited, setEdited] = useState<string | null>(null)
  const text = edited ?? base
  return (
    <EditorFile
      scope={scope}
      filePath={path}
      kind={kind}
      base={base}
      baseAbsent={baseAbsent}
      locked={false}
      text={text}
      setText={setEdited}
      setBase={setBase}
      readOnly={readOnly}
      cursor={{ line: 1, col: 1 }}
      width={1440}
      renderEditor={(o) => <textarea data-testid="ta" value={text} readOnly={o.readOnly} onChange={(e) => setEdited(e.target.value)} />}
    />
  )
}

const ta = (): HTMLTextAreaElement => screen.getByTestId('ta') as HTMLTextAreaElement
const type = (v: string): void => {
  fireEvent.change(ta(), { target: { value: v } })
}
const cmdS = (): void => {
  fireEvent.keyDown(ta(), { key: 's', metaKey: true })
}
const word = (): string => screen.getByTestId('ed-stat-save').textContent ?? ''
const dirty = (): boolean => screen.getByTestId('editor-file').hasAttribute('data-dirty')
const bar = (): HTMLElement => screen.getByTestId('ed-save-notice')
const click = (action: string): void => {
  fireEvent.click(bar().querySelector(`[data-action="${action}"]`) as HTMLElement)
}
const writes = (): { path: string; content: string; expect: string }[] => post.mock.calls.filter((c) => c[1] === 'fs/write-file').map((c) => c[2])
const conflict = (code: string, current?: object): DaemonHttpError => new DaemonHttpError('conflict', 409, code, null, { error: 'conflict', code, ...(current ? { current } : {}) })

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory()
  resetDraftsForTests()
  post.mockReset()
  get.mockReset()
  post.mockResolvedValue({ ok: true })
})
afterEach(() => cleanup())

describe('autosave, ⌘S and blur', () => {
  it('typing: the dot and "Saving in a moment"; after the idle it writes with expect and says Saved', async () => {
    render(<Harness initial={'# Quill\n'} />)
    expect(word()).toBe('Saved')
    type('# Quill\n\nNew words.\n')
    expect(word()).toBe('Saving in a moment')
    expect(dirty()).toBe(true)
    await waitFor(() => expect(writes()).toHaveLength(1), { timeout: 2000 })
    expect(writes()[0]).toEqual({ path: PATH, content: '# Quill\n\nNew words.\n', expect: await sha256Hex('# Quill\n') })
    await waitFor(() => expect(word()).toBe('Saved'))
    expect(dirty()).toBe(false)
  })

  it('⌘S saves now, and the next write expects the text it saved', async () => {
    render(<Harness initial="a" />)
    type('ab')
    cmdS()
    await waitFor(() => expect(writes()).toHaveLength(1), { timeout: 300 })
    await waitFor(() => expect(dirty()).toBe(false))
    type('abc')
    cmdS()
    await waitFor(() => expect(writes()).toHaveLength(2), { timeout: 300 })
    expect(writes()[1].expect).toBe(await sha256Hex('ab'))
  })

  it('leaving the editor saves', async () => {
    render(<Harness initial="a" />)
    type('ab')
    fireEvent.focusOut(ta(), { relatedTarget: null })
    await waitFor(() => expect(writes()).toHaveLength(1), { timeout: 300 })
  })

  it('a read-only file never writes', async () => {
    render(<Harness initial="a" readOnly="You can view this file but not edit it" />)
    expect(screen.getByTestId('editor-tab').getAttribute('data-mode')).toBe('read')
    fireEvent.keyDown(screen.getByTestId('editor-tab'), { key: 's', metaKey: true })
    expect(screen.queryByTestId('ed-stat-save')).toBeNull()
    await new Promise((r) => setTimeout(r, 1000))
    expect(writes()).toHaveLength(0)
  })
})

describe('a failed write', () => {
  it('first: the bar names the file and why, Retry; the dot and the words stay; the status says Not saved', async () => {
    post.mockRejectedValue(new DaemonHttpError('No space left on device (os error 28)', 500, null))
    render(<Harness initial="a" />)
    type('a mine')
    cmdS()
    await waitFor(() => expect(bar().textContent).toContain('Couldn’t save README.md: the drive is full.'))
    expect(bar().textContent).toContain('Your words are still here.')
    expect(bar().getAttribute('role')).toBe('alert')
    expect([...bar().querySelectorAll('[data-action]')].map((b) => b.textContent)).toEqual(['Retry'])
    expect(dirty()).toBe(true)
    expect(ta().value).toBe('a mine')
    expect(word()).toBe('Not saved')
    // No retry loop: autosave waits for a new edit (or Retry).
    await new Promise((r) => setTimeout(r, 1200))
    expect(writes()).toHaveLength(1)
    expect(bar().getAttribute('data-kind')).toBe('failed')
    expect(bar().textContent).not.toContain('second try')
  })

  it('second: "(second try)", what to do, Retry and Save a copy; the copy never overwrites', async () => {
    post.mockRejectedValue(new DaemonHttpError('No space left on device (os error 28)', 500, null))
    render(<Harness initial="a" />)
    type('a mine')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('failed'))
    click('retry')
    await waitFor(() => expect(bar().textContent).toContain('Still can’t save README.md (second try): the drive is full.'))
    expect(bar().textContent).toContain('Free up some space, then try again. Your words stay here.')
    expect([...bar().querySelectorAll('[data-action]')].map((b) => b.textContent)).toEqual(['Retry', 'Save a copy'])
    expect(dirty()).toBe(true)
    expect(word()).toBe('Not saved')
    post.mockReset()
    post.mockRejectedValueOnce(conflict('file_exists', { sha256: 'a'.repeat(64), content: 'x' })).mockResolvedValue({})
    click('save-copy')
    await waitFor(() => expect(bar().textContent).toContain('Saved your words as README (copy 2).md.'))
    expect(writes().map((w) => [w.path, w.expect])).toEqual([
      ['/ws/quill/README (copy).md', 'absent'],
      ['/ws/quill/README (copy 2).md', 'absent'],
    ])
    expect(dirty()).toBe(true)
  })

  it('Retry that lands: the bar goes, Saved', async () => {
    post.mockRejectedValueOnce(new Error('Failed to fetch'))
    render(<Harness initial="a" />)
    type('a mine')
    cmdS()
    await waitFor(() => expect(bar().textContent).toContain('the server can’t be reached'))
    click('retry')
    await waitFor(() => expect(word()).toBe('Saved'))
    expect(screen.queryByTestId('ed-save-notice')).toBeNull()
    expect(dirty()).toBe(false)
  })
})

describe('the disk check (409)', () => {
  const base = 'A\nB\nC\n'

  it('file_changed: merged three ways, both kept where both changed, said after the fact, then saved over theirs', async () => {
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'f'.repeat(64), content: 'A\nB theirs\nC\nD\n' }))
    render(<Harness initial={base} />)
    type('A\nB mine\nC\n')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('merged'))
    expect(ta().value).toBe('A\nB mine\nB theirs\nC\nD\n')
    expect(bar().textContent).toContain('Saved outside K2. Its changes are merged in.')
    expect(bar().textContent).toContain('K2 kept both versions for you to tidy')
    expect([...bar().querySelectorAll('[data-action]')].map((b) => b.textContent)).toEqual(['Compare', 'Undo their save', 'Use their version'])
    expect(bar().querySelector('[aria-label="Dismiss"]')).toBeTruthy()
    // The merged text saves, expecting their save.
    await waitFor(() => expect(writes()).toHaveLength(2), { timeout: 2000 })
    expect(writes()[1]).toMatchObject({ content: 'A\nB mine\nB theirs\nC\nD\n', expect: await sha256Hex('A\nB theirs\nC\nD\n') })
    await waitFor(() => expect(word()).toBe('Merged · Saved'))
  })

  it('Compare shows before against now in the text’s place; Close compare goes back', async () => {
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'f'.repeat(64), content: 'A\nB theirs\nC\n' }))
    render(<Harness initial={base} />)
    type('A\nB mine\nC\n')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('merged'))
    click('compare')
    expect((await screen.findByTestId('ed-compare')).textContent).toContain('Before external edit')
    expect(screen.queryByTestId('ta')).toBeNull()
    click('close-compare')
    expect(screen.queryByTestId('ed-compare')).toBeNull()
    expect(screen.getByTestId('ta')).toBeTruthy()
  })

  it('Use their version takes the file; Undo brings your words back. Undo their save puts your text back', async () => {
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'f'.repeat(64), content: 'A\nB theirs\nC\n' }))
    render(<Harness initial={base} />)
    type('A\nB mine\nC\n')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('merged'))
    const merged = ta().value
    click('use-theirs')
    expect(ta().value).toBe('A\nB theirs\nC\n')
    expect(bar().textContent).toContain('Using the version saved outside K2.')
    click('undo-use-theirs')
    expect(ta().value).toBe(merged)
    click('undo-theirs')
    expect(ta().value).toBe('A\nB mine\nC\n')
    expect(bar().textContent).toContain('Put back your text from before that save.')
  })

  it('file_missing: says so, keeps the words, stops autosave; Save it again writes expecting no file', async () => {
    post.mockRejectedValueOnce(conflict('file_missing'))
    render(<Harness initial={base} />)
    type('A\nB mine\nC\n')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('missing'))
    expect(bar().textContent).toContain('README.md was deleted or moved.')
    expect(bar().textContent).toContain('Your words are still here.')
    expect(word()).toBe('Not saved: the file was deleted or moved')
    expect(dirty()).toBe(true)
    type('A\nB mine, more\nC\n')
    await new Promise((r) => setTimeout(r, 1000))
    expect(writes()).toHaveLength(1)
    click('save-again')
    await waitFor(() => expect(writes()).toHaveLength(2))
    expect(writes()[1]).toMatchObject({ content: 'A\nB mine, more\nC\n', expect: 'absent' })
    await waitFor(() => expect(word()).toBe('Saved'))
  })

  it('file_exists: a new file met one someone else made; merged the same way', async () => {
    post.mockRejectedValueOnce(conflict('file_exists', { sha256: 'a'.repeat(64), content: 'Theirs.\n' }))
    render(<Harness initial="" baseAbsent />)
    type('Mine.\n')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('merged'))
    expect(writes()[0].expect).toBe('absent')
    expect(ta().value).toBe('Mine.\nTheirs.\n')
  })

  it('a guest gets the hash only: no merge, says so, Reload or Save a copy, no autosave', async () => {
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'f'.repeat(64) }))
    get.mockResolvedValue({ content: 'On disk now.\n' })
    render(<Harness initial={base} />)
    type('A\nB mine\nC\n')
    cmdS()
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('unmergeable'))
    expect(bar().textContent).toContain('K2 can’t see its text to merge.')
    expect([...bar().querySelectorAll('[data-action]')].map((b) => b.textContent)).toEqual(['Reload', 'Save a copy'])
    expect(ta().value).toBe('A\nB mine\nC\n')
    expect(word()).toBe('Not saved: the file changed on disk')
    await new Promise((r) => setTimeout(r, 1000))
    expect(writes()).toHaveLength(1)
    click('reload')
    await waitFor(() => expect(ta().value).toBe('On disk now.\n'))
    expect(dirty()).toBe(false)
    expect(word()).toBe('Saved')
  })
})

describe('drafts and the restore bar', () => {
  it('typing keeps a draft until the save lands', async () => {
    post.mockRejectedValue(new Error('No space left on device'))
    render(<Harness initial={'a\n'} />)
    type('a\nmine\n')
    await waitFor(async () => expect((await getDraft('local', PATH))?.text).toBe('a\nmine\n'))
    post.mockReset()
    post.mockResolvedValue({})
    cmdS()
    await waitFor(async () => expect(await getDraft('local', PATH)).toBeNull())
  })

  it('after a restart: the words come back with the bar, and save in a moment; Undo goes back to the file', async () => {
    await putDraft('local', PATH, 'a\nmine\n', 'a\n')
    render(<Harness initial={'a\n'} />)
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('restored'))
    expect(bar().textContent).toContain('Restored your unsaved changes from before K2 quit.')
    expect(bar().textContent).toContain('They save in a moment.')
    expect(ta().value).toBe('a\nmine\n')
    expect(word()).toBe('Saving in a moment')
    click('undo-restore')
    expect(ta().value).toBe('a\n')
    expect(screen.queryByTestId('ed-save-notice')).toBeNull()
    await new Promise((r) => setTimeout(r, 1000))
    expect(writes()).toHaveLength(0)
  })

  it('a restored draft saves on its own', async () => {
    await putDraft('local', PATH, 'a\nmine\n', 'a\n')
    render(<Harness initial={'a\n'} />)
    await waitFor(() => expect(writes()).toHaveLength(1), { timeout: 2000 })
    expect(writes()[0]).toMatchObject({ content: 'a\nmine\n', expect: await sha256Hex('a\n') })
  })
})

describe('the code editor saves the same way (the shared frame)', () => {
  it('autosave with expect, the failure bar keeping the dot, and ⌘S', async () => {
    post.mockRejectedValueOnce(new DaemonHttpError('No space left on device (os error 28)', 500, null))
    render(<Harness initial={'const a = 1\n'} kind="code" path="/ws/quill/src/a.ts" />)
    expect(screen.getByTestId('editor-tab').getAttribute('data-kind')).toBe('code')
    type('const a = 2\n')
    expect(word()).toBe('Saving in a moment')
    await waitFor(() => expect(bar().textContent).toContain('Couldn’t save a.ts: the drive is full.'), { timeout: 2000 })
    expect(writes()[0]).toMatchObject({ path: '/ws/quill/src/a.ts', expect: await sha256Hex('const a = 1\n') })
    expect(dirty()).toBe(true)
    expect(word()).toBe('Not saved')
    cmdS()
    await waitFor(() => expect(word()).toBe('Saved'))
    expect(writes()).toHaveLength(2)
  })
})

describe('the status line agrees with the bar', () => {
  it('never "Saved" while a bar says it isn’t', async () => {
    post.mockRejectedValue(new Error('Permission denied (os error 13)'))
    render(<Harness initial="a" />)
    type('ab')
    await act(async () => {
      cmdS()
    })
    await waitFor(() => expect(bar().getAttribute('data-kind')).toBe('failed'))
    expect(word()).not.toMatch(/^Saved/)
  })
})
