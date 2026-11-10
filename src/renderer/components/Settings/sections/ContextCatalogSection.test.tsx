// @vitest-environment jsdom
// Settings → Context Catalog: a failed read says so with Try again (it used
// to fall back to the built-in list as if it had loaded), packs are grouped
// into yours and K2's in plain words, New pack names a pack in a dialog and
// says what's next, and Delete asks first and says what happened.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('../ContextCatalogCreator', () => ({
  ContextCatalogCreator: ({ title }: { title: string }) => <div data-testid="pack-editor">{title}</div>,
}))

const role = vi.hoisted(() => ({ state: { kind: 'checking' } as { kind: string; role?: string } }))
vi.mock('./useActiveHostRole', () => ({ useActiveHostRole: () => ({ role: role.state, recheck: vi.fn() }) }))

const api = vi.hoisted(() => ({
  fetchContextCatalog: vi.fn(),
  createContextCatalogPack: vi.fn(),
  deleteContextCatalogPack: vi.fn(),
}))
vi.mock('@/lib/context-stack', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/lib/context-stack')>()
  return { ...real, ...api }
})

import { ContextCatalogSection } from './ContextCatalogSection'
import type { ContextCatalogEntry } from '@/lib/context-stack'
import { createFailureNext, shortNameFrom, shortNameProblem } from './context-catalog-copy'

const user = (slug: string, label: string, extra: Partial<ContextCatalogEntry> = {}): ContextCatalogEntry => ({
  id: `user:${slug}`,
  path: `.k2/context/catalog/${slug}.md`,
  label,
  source: `catalog:user:${slug}`,
  kind: 'static',
  dir: `/home/alex/.k2/context/catalog/packs/${slug}`,
  ...extra,
})
const WEEKLY = user('weekly-review', 'Weekly review', { description: 'How to write the Friday summary.', tags: ['team'] })
const HYGIENE: ContextCatalogEntry = {
  id: 'wiki:hygiene',
  path: '.k2/context/catalog/wiki-hygiene.md',
  label: 'Wiki hygiene',
  source: 'catalog:wiki-hygiene',
  kind: 'static',
  recommended: true,
  description: 'Standing orders for keeping .k2/wiki/ healthy — link, index, no orphans.',
}

const httpError = (code: string, hint: string): Error => new Error(JSON.stringify({ ok: false, error: { code, hint } }))

beforeEach(() => {
  for (const f of Object.values(api)) f.mockReset()
  role.state = { kind: 'checking' }
  api.fetchContextCatalog.mockResolvedValue([HYGIENE, WEEKLY])
})
afterEach(cleanup)

describe('ContextCatalogSection', () => {
  it('says the read failed, with the server’s reason, and Try again reads again', async () => {
    api.fetchContextCatalog.mockRejectedValueOnce(httpError('db_error', 'cannot read the catalog folder'))
    render(<ContextCatalogSection />)
    const card = await screen.findByTestId('context-catalog-error')
    expect(card.textContent).toContain('Couldn’t load the catalog')
    expect(card.textContent).toContain('The server said: cannot read the catalog folder.')
    expect(screen.queryByTestId('context-catalog-builtin')).toBeNull()
    fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
    expect(await screen.findByTestId('pack-row-user:weekly-review')).toBeTruthy()
    expect(api.fetchContextCatalog).toHaveBeenCalledTimes(2)
  })

  it('puts your packs and K2’s apart, in plain words', async () => {
    render(<ContextCatalogSection />)
    const yours = await screen.findByTestId('context-catalog-yours')
    expect(within(yours).getByText('Weekly review')).toBeTruthy()
    expect(within(yours).getByRole('button', { name: 'Edit Weekly review' })).toBeTruthy()
    const builtin = screen.getByTestId('context-catalog-builtin')
    const row = within(builtin).getByTestId('pack-row-wiki:hygiene')
    expect(row.textContent).toContain('Recommended')
    // The person-facing line, not the server's agent-facing one.
    expect(row.textContent).toContain('Asks agents to keep the wiki tidy')
    expect(row.textContent).not.toContain('.k2/wiki/')
    expect(within(builtin).queryByRole('button', { name: /Edit/ })).toBeNull()
  })

  it('says there are none of your own yet', async () => {
    api.fetchContextCatalog.mockResolvedValue([HYGIENE])
    render(<ContextCatalogSection />)
    expect((await screen.findByTestId('context-catalog-empty')).textContent).toContain('No packs of your own yet')
  })

  it('names a new pack in a dialog, makes the short name from it, and says what’s next', async () => {
    const made = user('on-call-handbook', 'On-call handbook')
    api.createContextCatalogPack.mockResolvedValue({ entry: made, dir: made.dir })
    render(<ContextCatalogSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'New pack' }))
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'On-call handbook' } })
    expect((screen.getByLabelText('Short name') as HTMLInputElement).value).toBe('on-call-handbook')
    fireEvent.change(screen.getByLabelText('Tags'), { target: { value: 'support, rota' } })
    api.fetchContextCatalog.mockResolvedValue([HYGIENE, WEEKLY, made])
    fireEvent.click(screen.getByRole('button', { name: 'Create pack' }))
    const done = await screen.findByTestId('pack-created-dialog')
    expect(api.createContextCatalogPack).toHaveBeenCalledWith({ id: 'user:on-call-handbook', label: 'On-call handbook', tags: ['support', 'rota'] })
    expect(done.textContent).toContain('On-call handbook is in the catalog')
    fireEvent.click(within(done).getByRole('button', { name: 'Later' }))
    expect((await screen.findByTestId('context-catalog-notice')).textContent).toContain('On-call handbook is in the catalog')
    expect(await screen.findByTestId('pack-row-user:on-call-handbook')).toBeTruthy()
  })

  it('opens the new pack to write it', async () => {
    const made = user('on-call-handbook', 'On-call handbook')
    api.createContextCatalogPack.mockResolvedValue({ entry: made, dir: made.dir })
    render(<ContextCatalogSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'New pack' }))
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'On-call handbook' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create pack' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Write it now' }))
    expect((await screen.findByTestId('pack-editor')).textContent).toBe('On-call handbook')
  })

  it('refuses a short name already in the catalog before asking the server', async () => {
    render(<ContextCatalogSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'New pack' }))
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Weekly review' } })
    expect(screen.getByText('A pack with this short name is already in the catalog. Change the short name.')).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Create pack' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('keeps the form open and says why when the server turns a new pack down', async () => {
    api.createContextCatalogPack.mockRejectedValue(httpError('bad_usage', 'reserved catalog id alias: user:manager'))
    render(<ContextCatalogSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'New pack' }))
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Manager' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create pack' }))
    const note = await screen.findByTestId('dialog-error')
    expect(note.textContent).toContain('Couldn’t create Manager.')
    expect(note.textContent).toContain('K2 keeps this short name for its own packs.')
    expect(note.textContent).toContain('The server said: reserved catalog id alias: user:manager.')
    expect(screen.getByTestId('new-pack-dialog')).toBeTruthy()
  })

  it('asks before deleting, then says what happened', async () => {
    api.deleteContextCatalogPack.mockResolvedValue(undefined)
    render(<ContextCatalogSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'More actions for Weekly review' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete pack/ }))
    const dialog = screen.getByTestId('delete-pack-dialog')
    expect(api.deleteContextCatalogPack).not.toHaveBeenCalled()
    api.fetchContextCatalog.mockResolvedValue([HYGIENE])
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete pack' }))
    await waitFor(() => expect(api.deleteContextCatalogPack).toHaveBeenCalledWith('user:weekly-review'))
    expect((await screen.findByTestId('context-catalog-notice')).textContent).toContain('Weekly review was deleted from the catalog')
    expect(screen.queryByTestId('pack-row-user:weekly-review')).toBeNull()
  })

  it('keeps the delete dialog open with the reason when it fails', async () => {
    api.deleteContextCatalogPack.mockRejectedValue(httpError('db_error', 'resource busy'))
    render(<ContextCatalogSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'More actions for Weekly review' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete pack/ }))
    fireEvent.click(within(screen.getByTestId('delete-pack-dialog')).getByRole('button', { name: 'Delete pack' }))
    const note = await screen.findByTestId('dialog-error')
    expect(note.textContent).toContain('It’s still in the catalog.')
    expect(note.textContent).toContain('resource busy')
    expect(screen.getByTestId('pack-row-user:weekly-review')).toBeTruthy()
  })

  it('turns the changes off for a Member, and says who can make them', async () => {
    role.state = { kind: 'known', role: 'member' }
    render(<ContextCatalogSection />)
    const add = await screen.findByRole('button', { name: 'New pack' })
    expect((add as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Edit Weekly review' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('context-catalog-yours').textContent).toContain('Only an Owner or Admin can add, change or delete them.')
  })
})

describe('context-catalog-copy', () => {
  it('makes a short name from a name', () => {
    expect(shortNameFrom('Weekly review')).toBe('weekly-review')
    expect(shortNameFrom('  Café: notes & tips!  ')).toBe('cafe-notes-tips')
    expect(shortNameFrom('???')).toBe('')
  })

  it('checks a short name the way the server does', () => {
    expect(shortNameProblem('weekly-review', [])).toBeNull()
    expect(shortNameProblem('Weekly', [])).toMatch(/lowercase/)
    expect(shortNameProblem('-x', [])).toMatch(/starting with a letter/)
    expect(shortNameProblem('weekly', ['user:weekly'])).toMatch(/already in the catalog/)
  })

  it('turns the server’s reasons into a next step', () => {
    expect(createFailureNext('catalog pack already exists: x')).toMatch(/Pick another short name/)
    expect(createFailureNext('reserved catalog path stem: manager')).toMatch(/its own packs/)
    expect(createFailureNext('disk full')).toBe('Nothing was added. Try again in a moment.')
  })
})
