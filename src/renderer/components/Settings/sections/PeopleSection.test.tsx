// @vitest-environment jsdom
//
// Settings → K2 Server → People. Principals (username + full name).
// Not Server Access's ConnectTab 'people', not skin-access.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))

import { PeopleSection, PEOPLE_MANIFEST, parsePeople } from './PeopleSection'
import { SECTION_LABELS } from '../searchManifest'

const dir = dirname(fileURLToPath(import.meta.url))

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
})

describe('PEOPLE_MANIFEST', () => {
  it('is K2 Server People, not skin-access and not Server Access', () => {
    expect(PEOPLE_MANIFEST.every((e) => e.section === 'people')).toBe(true)
    expect(PEOPLE_MANIFEST.some((e) => e.section === 'skin-access')).toBe(false)
    expect(SECTION_LABELS.people).toBe('People')
    expect(SECTION_LABELS['k2-access']).toBe('Server Access')
    expect(SECTION_LABELS['skin-access']).toBe('Apps')
  })
})

describe('PeopleSection', () => {
  it('lists username and full name from principals and does not fetch projects', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route !== 'skin/users') throw new Error(`unexpected GET ${route}`)
      return {
        users: [
          { username: 'ada', fullName: 'Ada Lovelace' },
          { username: 'bob', fullName: null },
        ],
      }
    })
    render(<PeopleSection />)
    await waitFor(() => {
      expect(screen.getByText('ada')).toBeTruthy()
    })
    expect(screen.getByRole('heading', { level: 2, name: 'People' })).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'Server Access' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Apps' })).toBeNull()
    const ada = screen.getByLabelText('ada full name') as HTMLInputElement
    const bob = screen.getByLabelText('bob full name') as HTMLInputElement
    expect(ada.value).toBe('Ada Lovelace')
    expect(bob.value).toBe('')
    expect(h.daemonCliGet).toHaveBeenCalledWith('skin/users')
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'projects/list')).toBe(true)
  })

  it('owner save and clear post skin/users/full-name', async () => {
    h.daemonCliGet.mockResolvedValue({
      users: [{ username: 'ada', fullName: 'Ada Lovelace' }],
    })
    h.daemonCliPost.mockResolvedValue({ username: 'ada', fullName: 'Ada/Lovelace: MD' })
    render(<PeopleSection />)
    const input = await screen.findByLabelText('ada full name')
    fireEvent.change(input, { target: { value: 'Ada/Lovelace: MD' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save ada full name' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', {
        username: 'ada',
        fullName: 'Ada/Lovelace: MD',
      })
    })

    h.daemonCliGet.mockResolvedValue({ users: [{ username: 'ada', fullName: 'Ada Lovelace' }] })
    fireEvent.click(screen.getByRole('button', { name: 'Clear ada full name' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', {
        username: 'ada',
        fullName: '',
      })
    })
  })

  it('does not call fetchProjects and is not the Connect people tab', () => {
    const src = readFileSync(resolve(dir, 'PeopleSection.tsx'), 'utf8')
    expect(src).not.toContain('fetchProjects')
    expect(src).not.toContain('skin-access')
    expect(src).not.toContain("section: 'k2-access'")
    const shell = readFileSync(resolve(dir, 'K2ConnectSettingsShell.tsx'), 'utf8')
    expect(shell).toContain("title: 'Server Access'")
    expect(shell).toContain("if (section === 'k2-access') return 'people'")
    expect(shell).not.toContain('PeopleSection')
    const parsed = parsePeople({
      users: [{ username: 'ada', full_name: 'Ada Lovelace' }, { username: '' }],
    })
    expect(parsed).toEqual([{ username: 'ada', fullName: 'Ada Lovelace' }])
  })
})
