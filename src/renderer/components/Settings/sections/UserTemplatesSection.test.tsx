// @vitest-environment jsdom
//
// Settings → Access → User Templates: open on a template (never an empty
// pane), make one, say what it gives in plain words, apply it to a person
// with a confirmation and a way to see them, and confirm a delete.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import { UserTemplatesSection, USER_TEMPLATES_MANIFEST } from './UserTemplatesSection'
import { takePersonHandoff } from './user-access-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { menuOptions, pickValue, waitForMenuOptions } from '@/test-utils/setting-dropdown'

type Json = Record<string, unknown>

const ROLES = [
  { id: 'role-staff', name: 'Staff', appId: 'svc-shop', roomAccess: [{ handle: 'atlas', caps: ['thread:read'] }] },
  { id: 'role-reader', name: 'Reader', appId: null, roomAccess: [{ handle: 'notes', caps: ['thread:read'] }, { handle: 'atlas', caps: ['thread:read'] }] },
]

let templates: Json[]
let users: Json[]
let fail: Error | null

function wire(): void {
  h.daemonCliGet.mockImplementation(async (route: string, q?: Record<string, string>) => {
    if (fail) throw fail
    if (route === 'skin/templates') return { templates }
    if (route === 'skin/roles') return { roles: ROLES }
    if (route === 'skin/users') return { users }
    if (route === 'projects/list') return [{ id: 'ws-1' }]
    if (route === 'publish/list') return q?.project === 'ws-1' ? { services: [{ id: 'svc-shop', name: 'Shop', kind: 'skin' }] } : { services: [] }
    throw new Error(`unexpected GET ${route}`)
  })
}

const confirmSpy = vi.fn(async () => true)

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  fail = null
  templates = [
    {
      id: 't-front',
      name: 'Front desk',
      lines: [
        { id: 'l1', templateId: 't-front', kind: 'app', roleId: 'role-staff', position: 0 },
        { id: 'l2', templateId: 't-front', kind: 'app', roleId: 'role-reader', position: 1 },
      ],
    },
    { id: 't-empty', name: 'Night shift', lines: [] },
  ]
  users = [{ id: 'p-ada', username: 'ada', fullName: 'Ada Lovelace' }, { id: 'p-bob', username: 'bob' }]
  wire()
  confirmSpy.mockReset()
  confirmSpy.mockResolvedValue(true)
  useConfirmDialogStore.setState({ confirm: confirmSpy })
})

describe('UserTemplatesSection', () => {
  it('says what it is in plain words', () => {
    expect(USER_TEMPLATES_MANIFEST[0].description).toContain('Starting sets of access for new people')
  })

  it('opens on the first template and says what each line gives', async () => {
    render(<UserTemplatesSection />)
    expect(await screen.findByRole('heading', { level: 2, name: 'Front desk' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Open template Front desk' }).textContent).toContain('2 Apps')
    expect(screen.getByRole('button', { name: 'Open template Night shift' }).textContent).toContain('Nothing in it yet')
    const lines = screen.getByTestId('template-lines')
    await waitFor(() => expect(within(lines).getByTestId('template-line-l1').textContent).toContain('Shop, as Staff'))
    expect(within(lines).getByTestId('template-line-l1').textContent).toContain('Talks to the atlas agent.')
    expect(within(lines).getByTestId('template-line-l2').textContent).toContain('An App you pick, as Reader')
    expect(screen.queryByText(/grant|principal/i)).toBeNull()
  })

  it('shows loading, then a failed read with its reason and Try again', async () => {
    fail = new Error('database is locked')
    render(<UserTemplatesSection />)
    expect(screen.getByTestId('templates-loading')).toBeTruthy()
    const err = await screen.findByTestId('templates-error')
    expect(err.textContent).toContain('Couldn’t load templates')
    expect(err.textContent).toContain('The server said: database is locked.')
    fail = null
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }))
    expect(await screen.findByRole('heading', { level: 2, name: 'Front desk' })).toBeTruthy()
  })

  it('with none yet explains templates and makes one: name, create, then add an App', async () => {
    templates = []
    h.daemonCliPost.mockImplementation(async (route: string, body: Json) => {
      if (route === 'skin/templates') {
        const t = { id: 't-new', name: body.name, lines: [] as Json[] }
        templates.push(t)
        return t
      }
      if (route === 'skin/templates/lines') {
        const line = { id: 'l-new', templateId: body.templateId, kind: body.kind, roleId: body.roleId, position: 0 }
        ;(templates[0].lines as Json[]).push(line)
        return line
      }
      throw new Error(`unexpected POST ${route}`)
    })
    render(<UserTemplatesSection />)
    const empty = await screen.findByTestId('templates-empty')
    expect(empty.textContent).toContain('No templates yet')
    fireEvent.click(within(empty).getByRole('button', { name: 'New template' }))
    const create = screen.getByRole('button', { name: 'Create template' }) as HTMLButtonElement
    expect(create.disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('New template name'), { target: { value: 'Night shift' } })
    fireEvent.click(create)
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/templates', { name: 'Night shift' }))
    expect(await screen.findByRole('heading', { level: 2, name: 'Night shift' })).toBeTruthy()
    expect(screen.getByText('Created. Now add the Apps it gives, then apply it to someone.')).toBeTruthy()
    // A new template opens on adding its first App.
    const add = screen.getByTestId('template-add-line')
    expect(menuOptions(within(add).getByLabelText('Role to add')).map((o) => o.label)).toEqual(['Staff (Shop only)', 'Reader'])
    pickValue(within(add).getByLabelText('Role to add'), 'role-reader')
    fireEvent.click(within(add).getByRole('button', { name: 'Add to template' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/templates/lines', { templateId: 't-new', kind: 'app', roleId: 'role-reader' }))
    expect(await within(screen.getByTestId('template-lines')).findByText('Saved')).toBeTruthy()
    expect(screen.getByTestId('template-line-l-new').textContent).toContain('An App you pick, as Reader')
  })

  it('applies: pick a person and the App for a role that works anywhere, then says what they got', async () => {
    h.daemonCliPost.mockResolvedValue({ grants: [] })
    render(<UserTemplatesSection />)
    const card = await screen.findByTestId('template-apply')
    const apply = (): HTMLButtonElement => within(screen.getByTestId('template-apply-row')).getAllByRole('button')[0] as HTMLButtonElement
    expect(apply().disabled).toBe(true)
    expect(menuOptions(within(card).getByLabelText('Apply template to')).map((o) => o.label)).toEqual(['Ada Lovelace (ada)', 'bob'])
    pickValue(within(card).getByLabelText('Apply template to'), 'p-ada')
    // The Staff role is Shop's own, so only Reader asks where.
    expect(within(card).queryByLabelText('App for Staff')).toBeNull()
    const where = within(card).getByLabelText('App for Reader')
    await waitForMenuOptions(where, (o) => expect(o.map((x) => x.label)).toEqual(['K2 on this server', 'Shop']))
    expect(apply().disabled).toBe(true)
    pickValue(where, 'host')
    expect(apply().textContent).toBe('Apply to ada')
    expect(apply().disabled).toBe(false)
    fireEvent.click(apply())
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/templates/apply', {
        principalId: 'p-ada',
        templateId: 't-front',
        lines: [
          { lineId: 'l1', targetId: 'svc-shop' },
          { lineId: 'l2', targetId: 'host' },
        ],
      }),
    )
    const done = await screen.findByText('ada can now use Shop and K2 on this server.')
    expect(done).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'See ada in User Access' }))
    expect(useSettingsStore.getState().activeSection).toBe('people')
    expect(takePersonHandoff()).toBe('ada')
  })

  it('a failed apply says why and keeps the choices', async () => {
    h.daemonCliPost.mockRejectedValue(new Error("role 'Staff' is for app 'svc-shop', not 'host'"))
    render(<UserTemplatesSection />)
    const card = await screen.findByTestId('template-apply')
    pickValue(within(card).getByLabelText('Apply template to'), 'p-bob')
    await waitForMenuOptions(within(card).getByLabelText('App for Reader'), (o) => expect(o.length).toBe(2))
    pickValue(within(card).getByLabelText('App for Reader'), 'svc-shop')
    fireEvent.click(within(screen.getByTestId('template-apply-row')).getByRole('button', { name: 'Apply to bob' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe("Couldn’t save: role 'Staff' is for app 'svc-shop', not 'host'. Nothing changed; try again.")
    expect(within(card).getByLabelText('Apply template to').textContent).toBe('bob')
  })

  it('with no people yet, Apply points to User Access', async () => {
    users = []
    render(<UserTemplatesSection />)
    const card = await screen.findByTestId('template-apply')
    expect(card.textContent).toContain('No people yet')
    fireEvent.click(within(card).getByRole('button', { name: 'Open User Access' }))
    expect(useSettingsStore.getState().activeSection).toBe('people')
  })

  it('delete confirms first and says people keep what it gave them', async () => {
    h.daemonCliPost.mockResolvedValue({ success: true })
    render(<UserTemplatesSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'More actions for Front desk' }))
    confirmSpy.mockResolvedValueOnce(false)
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete template/ }))
    await waitFor(() =>
      expect(confirmSpy).toHaveBeenCalledWith(
        expect.objectContaining({ title: 'Delete Front desk?', message: 'The template goes away. People it was applied to keep everything it gave them.', destructive: true }),
      ),
    )
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    templates = templates.slice(1)
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Front desk' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete template/ }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/templates/delete', { id: 't-front' }))
    expect(await screen.findByRole('heading', { level: 2, name: 'Night shift' })).toBeTruthy()
  })

  it('rename saves the new name', async () => {
    h.daemonCliPost.mockResolvedValue({})
    render(<UserTemplatesSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'More actions for Front desk' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Rename/ }))
    fireEvent.change(screen.getByLabelText('Template Front desk name'), { target: { value: 'Reception' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save name' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/templates/update', { id: 't-front', name: 'Reception' }))
  })
})
