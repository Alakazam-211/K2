// @vitest-environment jsdom
//
// Save as template (prd-agent-templates-v1 §7; the T5–T7 contract): the row in
// Settings → Workspaces → an agent opens a sheet with the id (a slug of the
// name), name, summary and the wiki notes (none picked); Next is the dry run,
// whose files, context, suggestions, tool, heartbeats and skipped items are
// drawn; Save writes and says what was saved. Each refusal code has its own
// words (a secret by file and line, never the secret), and an older server's
// 404 says it can't save templates yet and stops offering the button.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliPost: vi.fn(),
  fetchWikiIndex: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: vi.fn(),
    daemonCliPostQuery: vi.fn(),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})
vi.mock('@/components/Wiki/wiki-api', () => ({
  fetchWikiIndex: (p: string) => h.fetchWikiIndex(p),
  isWikiArticleNode: (n: { exists: boolean; kind?: string }) => n.exists && (n.kind ?? 'note') === 'note',
}))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { SAVE_TEMPLATE_COPY, SaveTemplateRow, SaveTemplateSheet, formatBytes, idProblem } from './SaveTemplateSheet'
import { SAVE_ERROR_COPY, SAVE_UNSUPPORTED_TEXT } from '@/lib/agent-templates-api'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { primaryScope } from '@/kessel/server-scope'
import { resetAgentTemplatesForTests, useAgentTemplatesStore } from '@/stores/agent-templates'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const PROJECT = { path: '/home/r/agents/max', name: 'Max Writer' }

function result(dryRun: boolean): Record<string, unknown> {
  return {
    ok: true,
    dryRun,
    id: 'max-writer',
    version: 2,
    author: 'user:rosson',
    path: '~/.k2/agent-templates/max-writer/v2',
    hash: 'sha256:x',
    files: [
      { path: 'template.toml', bytes: 812 },
      { path: 'ROLE.md', bytes: 2048 },
      { path: 'wiki/Voice and style.md', bytes: 300 },
    ],
    catalog: ['wiki:index'],
    suggest: ['compute.use'],
    tool: 'codex',
    heartbeats: ['daily-digest'],
    skipped: [{ what: 'skill', name: 'k2-cli', why: 'k2_managed' }],
  }
}

function note(id: string): Record<string, unknown> {
  return { id, title: id, aliases: [], tags: [], path: `.k2/wiki/${id}`, exists: true, kind: 'note' }
}

beforeEach(() => {
  resetAgentTemplatesForTests()
  h.daemonCliPost.mockReset()
  h.fetchWikiIndex.mockReset()
  h.fetchWikiIndex.mockResolvedValue({
    nodes: [note('Voice and style.md'), note('Home.md'), { ...note('Ghost.md'), exists: false }],
    links: [],
  })
  h.daemonCliPost.mockImplementation(async (route: string, body: { dryRun: boolean }) => {
    if (route === 'agent-templates/save') return result(body.dryRun)
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => {
  cleanup()
})

const onClose = vi.fn()

async function openSheet(): Promise<void> {
  onClose.mockClear()
  render(<SaveTemplateSheet project={PROJECT} onClose={onClose} />)
  await waitFor(() => expect(screen.getByTestId('save-template-note-Home.md')).toBeTruthy())
}

async function click(testId: string): Promise<void> {
  await act(async () => {
    fireEvent.click(screen.getByTestId(testId))
  })
}

function refuse(e: unknown): void {
  h.daemonCliPost.mockRejectedValue(e)
}

describe('helpers', () => {
  it('the id is a slug', () => {
    expect(idProblem('my-writer')).toBeNull()
    expect(idProblem('My Writer')).not.toBeNull()
    expect(idProblem('')).not.toBeNull()
  })
  it('sizes read plainly', () => {
    expect(formatBytes(812)).toBe('812 B')
    expect(formatBytes(2048)).toBe('2.0 KB')
  })
})

describe('SaveTemplateSheet', () => {
  it('starts with the id from the name and no wiki notes picked', async () => {
    await openSheet()
    expect(document.activeElement?.textContent).toBe(SAVE_TEMPLATE_COPY.details)
    expect((screen.getByTestId('save-template-id') as HTMLInputElement).value).toBe('max-writer')
    expect((screen.getByTestId('save-template-name') as HTMLInputElement).value).toBe('Max Writer')
    expect(screen.getByLabelText(/Summary/)).toBeTruthy()
    const boxes = screen.getAllByRole('checkbox') as HTMLInputElement[]
    expect(boxes.map((b) => b.checked)).toEqual([false, false])
    expect(screen.queryByTestId('save-template-note-Ghost.md')).toBeNull()
    expect(h.fetchWikiIndex).toHaveBeenCalledWith(PROJECT.path)
  })

  it('a bad id holds Next', async () => {
    await openSheet()
    fireEvent.change(screen.getByTestId('save-template-id'), { target: { value: 'Not A Slug' } })
    await click('save-template-next')
    expect(screen.getByTestId('save-template-id-problem')).toBeTruthy()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('Next is the dry run; the review draws files, context, suggest, tool, heartbeats and skipped', async () => {
    await openSheet()
    fireEvent.click(screen.getByTestId('save-template-note-Voice and style.md'))
    fireEvent.change(screen.getByTestId('save-template-summary'), { target: { value: ' Writes posts ' } })
    await click('save-template-next')
    expect(h.daemonCliPost).toHaveBeenCalledWith('agent-templates/save', {
      workspace: PROJECT.path,
      id: 'max-writer',
      name: 'Max Writer',
      summary: 'Writes posts',
      wiki: ['Voice and style.md'],
      dryRun: true,
    })
    await waitFor(() => expect(screen.getByTestId('save-template-files')).toBeTruthy())
    expect(document.activeElement?.textContent).toBe(SAVE_TEMPLATE_COPY.review)
    expect(screen.getAllByTestId('save-template-file').map((r) => r.textContent)).toEqual([
      'template.toml812 B',
      'ROLE.md2.0 KB',
      'wiki/Voice and style.md300 B',
    ])
    expect(screen.getByTestId('save-template-catalog').textContent).toContain('wiki:index')
    expect(screen.getByTestId('save-template-suggest').textContent).toContain('compute.use')
    expect(screen.getByTestId('save-template-tool').textContent).toContain('codex')
    expect(screen.getByTestId('save-template-heartbeats').textContent).toContain('daily-digest')
    const skip = screen.getByTestId('save-template-skip').textContent
    expect(skip).toContain('Skill')
    expect(skip).toContain('k2-cli')
    expect(skip).toContain('K2 provides it')
    expect(screen.getByTestId('save-template-review-head').textContent).toContain('Byrosson')
  })

  it('Save writes and says what was saved', async () => {
    await openSheet()
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-save')).toBeTruthy())
    await click('save-template-save')
    expect(h.daemonCliPost.mock.calls[1][1]).toMatchObject({ dryRun: false, id: 'max-writer' })
    await waitFor(() => expect(screen.getByTestId('save-template-done')).toBeTruthy())
    expect(screen.getByTestId('save-template-done').textContent).toContain('max-writer, version 2')
    fireEvent.click(screen.getByTestId('save-template-close'))
    expect(onClose).toHaveBeenCalled()
  })

  it.each([
    ['id_reserved', 409, SAVE_ERROR_COPY.id_reserved],
    ['id_taken', 409, SAVE_ERROR_COPY.id_taken],
    ['bad_id', 400, SAVE_ERROR_COPY.bad_id],
  ])('%s: its own words, back on the id', async (code, status, text) => {
    await openSheet()
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-save')).toBeTruthy())
    refuse(new DaemonHttpError('refused', status, code))
    await click('save-template-save')
    await waitFor(() => expect(screen.getByTestId('save-template-error')).toBeTruthy())
    expect(screen.getByTestId('save-template-error').textContent).toBe(text)
    expect(screen.getByTestId('save-template-id').getAttribute('aria-invalid')).toBe('true')
  })

  it.each([
    ['own_workspace_only', 403, SAVE_ERROR_COPY.own_workspace_only],
    ['no_workspace', 404, SAVE_ERROR_COPY.no_workspace],
  ])('%s: its own words', async (code, status, text) => {
    refuse(new DaemonHttpError('refused', status, code))
    await openSheet()
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-error')).toBeTruthy())
    expect(screen.getByTestId('save-template-error').textContent).toBe(text)
  })

  it('too_large: its own words and the server’s', async () => {
    refuse(new DaemonHttpError('ROLE.md is over 64 KiB', 413, 'too_large'))
    await openSheet()
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-error')).toBeTruthy())
    const text = screen.getByTestId('save-template-error').textContent
    expect(text).toContain(SAVE_ERROR_COPY.too_large)
    expect(text).toContain('The server said: ROLE.md is over 64 KiB.')
  })

  it('secret_found: names the file and line, never the secret', async () => {
    const body = { ok: false, error: { code: 'secret_found', hint: 'context/keys.md line 3 has sk-ant-SECRET', file: 'context/keys.md', line: 3 } }
    refuse(new DaemonHttpError(body.error.hint, 422, 'secret_found', body.error, body))
    await openSheet()
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-error')).toBeTruthy())
    const text = screen.getByTestId('save-template-error').textContent ?? ''
    expect(text).toContain('context/keys.md, line 3')
    expect(text).not.toContain('sk-ant')
    expect(text).not.toContain('The server said')
  })

  it('an older server (404, no code): says it can’t save yet, and the row stops offering it', async () => {
    refuse(new DaemonHttpError('route not found', 404, null))
    render(<SaveTemplateRow project={PROJECT} />)
    fireEvent.click(screen.getByTestId('save-template-open'))
    await waitFor(() => expect(screen.getByTestId('save-template-next')).toBeTruthy())
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-unsupported')).toBeTruthy())
    expect(screen.getByTestId('save-template-unsupported').textContent).toBe(SAVE_UNSUPPORTED_TEXT)
    expect(useAgentTemplatesStore.getState().saveUnsupported[primaryScope().hostKey]).toBe(true)
    fireEvent.click(screen.getByTestId('save-template-cancel'))
    expect(screen.queryByTestId('save-template-open')).toBeNull()
    expect(screen.getByText(SAVE_UNSUPPORTED_TEXT)).toBeTruthy()
  })

  it('the row says what it does, and its button opens the sheet', async () => {
    render(<SaveTemplateRow project={PROJECT} />)
    const row = screen.getByTestId('save-template-row')
    expect(row.textContent).toContain(SAVE_TEMPLATE_COPY.rowLabel)
    expect(row.textContent).toContain(SAVE_TEMPLATE_COPY.rowHint)
    expect(screen.getByTestId('save-template-open').textContent).toBe(SAVE_TEMPLATE_COPY.button)
    fireEvent.click(screen.getByTestId('save-template-open'))
    await waitFor(() => expect(screen.getByTestId('save-template-sheet')).toBeTruthy())
  })

  it('closing is held while it saves', async () => {
    await openSheet()
    await click('save-template-next')
    await waitFor(() => expect(screen.getByTestId('save-template-save')).toBeTruthy())
    let release: (v: unknown) => void = () => {}
    h.daemonCliPost.mockImplementation(() => new Promise((r) => (release = r)))
    await click('save-template-save')
    expect((screen.getByTestId('dialog-close') as HTMLButtonElement).disabled).toBe(true)
    await act(async () => {
      release(result(false))
    })
    await waitFor(() => expect(screen.getByTestId('save-template-done')).toBeTruthy())
  })
})
