// @vitest-environment jsdom
// Settings → Apps → Add an App (prd-one-click-suite-apps-v1 R1–R5): the
// catalog rows, the deploy card's live address check and suggestion, Deploy
// starting the daemon job, its steps, a wait, the owner's Approve with a
// fresh vault confirm, and a job in flight showing again when Settings
// reopens. Until Sew publishes the Seeds nothing is pinned, and the pane
// says so; a server with Apps off or an older K2 gets a sentence, not an
// error. Every answer is the daemon's (mocked here).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  entries: [] as unknown[],
  job: null as unknown,
  role: 'owner' as 'owner' | 'admin' | 'member' | null,
  taken: false,
  checkFails: false,
  off: false,
  old: false,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (route === 'app/catalog') {
        if (h.off) throw new Error(JSON.stringify({ error: { code: 'apps_disabled', hint: 'Apps (beta) are off' } }))
        if (h.old) {
          const { DaemonHttpError } = await import('@/lib/daemon-http-error')
          throw new DaemonHttpError('not_found', 404, null)
        }
        return { ok: true, entries: h.entries }
      }
      if (route === 'app/catalog/check-address') {
        if (h.checkFails) throw new Error('the tunnel config could not be read')
        if (h.taken && params?.address === 'chat') {
          return {
            ok: false,
            app: 'hearth-2',
            error: {
              code: 'address_taken',
              hint: "'chat' is used by the App hearth. Pick another address (suggested: hearth-2-chat): k2 app install --catalog sew/hearth --address hearth-2-chat",
              text: "'chat' is already used by the App hearth. Try hearth-2-chat instead",
              suggestion: 'hearth-2-chat',
            },
          }
        }
        return { ok: true, app: 'hearth', address: { kind: 'label', label: params?.address, url: `https://${params?.address}.acme.example.test` }, tier: { pro: true } }
      }
      if (route === 'app/catalog/job') return { ok: true, job: h.job }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route === 'app/catalog/install') return { ok: true, job: h.job }
      if (route === 'vault/confirm/start') return { methods: ['local-auth'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-1', expiresIn: 60 }
      if (route === 'app/approve') return { ok: true, status: 'approved', request: { id: body.request } }
      return { ok: true }
    }),
    daemonCliPostQuery: vi.fn(),
  }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => h.role) }))

import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { AppsCatalogSection, nothingPublished } from './AppsCatalogSection'
import { isActive, plainText, stepLabel, waitText, type CatalogJob } from './app-catalog-api'

function project(id: string, name: string): ProjectWithWorkspaces {
  return { id, name, path: `/ws/${name}`, color: '#22c55e', iconUrl: null, handle: name, workspaces: [] } as unknown as ProjectWithWorkspaces
}

const STEPS = ['download', 'check', 'install', 'address', 'deploy', 'approve', 'sew_setup', 'healthy', 'ready']

function job(state: CatalogJob['state'], step: string, over: Partial<CatalogJob> = {}): CatalogJob {
  const at = STEPS.indexOf(step)
  return {
    id: 'sj-1',
    entry: 'sew/hearth',
    version: '0.2.4',
    variant: 'k2',
    app: 'hearth',
    address: { kind: 'suite_label', label: 'chat', url: 'https://chat.acme.example.test' },
    step,
    state,
    steps: STEPS.map((s, i) => ({ step: s, state: state === 'done' || i < at ? 'done' : i === at ? state : 'todo' })),
    ...over,
  }
}

const HEARTH = {
  entry: 'sew/hearth',
  name: 'Hearth',
  publisher: { id: 'sew', name: 'Sew', source: 'https://github.com/Alakazam-211/sew-releases/releases' },
  summary: "Chat and voice for your community, on this server's chat.",
  license: 'MIT OR Apache-2.0',
  verified: 'Verified by K2',
  family: 'chat',
  defaultAddress: { label: 'chat', url: 'https://chat.acme.example.test' },
  onePerBox: false,
  version: '0.2.4',
  variant: 'k2',
  willDo: ['Runs as a web App on this server, in its own protected space', 'Joins voice and video calls'],
  approves: [{ kind: 'boxService', name: 'sew-api', why: 'This App can talk to the chat service on this server, for chat' }],
  canDeploy: true,
  reasons: [],
  alreadyDeployed: null,
  installed: [],
  jobs: [],
}

const MEET_BLOCKED = {
  ...HEARTH,
  entry: 'sew/meet',
  name: 'Meet',
  family: 'meet',
  defaultAddress: { label: 'meet', url: null },
  version: null,
  verified: null,
  canDeploy: false,
  reasons: [{ code: 'catalog_not_pinned', text: 'no version of Meet is pinned in this K2 yet' }],
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.entries = [HEARTH, MEET_BLOCKED]
  h.job = job('running', 'download')
  h.role = 'owner'
  h.taken = false
  h.checkFails = false
  h.off = false
  h.old = false
  useProjectsStore.setState({ projects: [project('ws-ops', 'ops'), project('ws-sales', 'sales')], activeProjectId: 'ws-sales' })
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('helpers', () => {
  it('names the steps and the waits', () => {
    expect(stepLabel('sew_setup')).toBe('Chat setup')
    expect(isActive(job('waiting', 'approve'))).toBe(true)
    expect(isActive(job('done', 'ready'))).toBe(false)
    const w = job('waiting', 'address', { waiting: { reason: 'waiting_sew_rights', requests: [], text: 'turn it on', hint: 'k2 agent access set ws chat_manage on' } })
    // The server's words and its command stay out of the sheet's own words.
    expect(waitText(w)).toContain('Waiting for permission to set up chat and voice')
    expect(waitText(w)).not.toContain('turn it on')
    expect(waitText(w)).not.toContain('k2 agent')
    expect(waitText(job('running', 'check'))).toBeNull()
  })

  it('reads the plain sentence, never the command alone', () => {
    // A server that splits its words: the sentence, even when the command is there too.
    expect(plainText({ text: 'Couldn’t download it', hint: 'k2 app catalog retry sj-1' })).toBe('Couldn’t download it')
    expect(plainText({ text: null, hint: 'k2 app catalog retry sj-1' })).toBeNull()
    // An older server sent only the sentence, as `hint`.
    expect(plainText({ hint: 'github.com timed out' })).toBe('github.com timed out')
    expect(plainText(null)).toBeNull()
  })
})

describe('AppsCatalogSection', () => {
  it('shows each entry and why one cannot deploy; one pinned entry is not "nothing published"', async () => {
    render(<AppsCatalogSection />)
    await screen.findByTestId('catalog-entry-sew/hearth')
    expect(screen.getByRole('heading', { level: 2, name: 'Add an App' })).toBeTruthy()
    // Only the entry with a pinned, published version shows the badge.
    expect(screen.getAllByTestId('catalog-verified')).toHaveLength(1)
    expect(screen.getByTestId('catalog-entry-sew/hearth').querySelector('[data-testid="catalog-verified"]')?.textContent).toBe('Verified by K2')
    expect(screen.getByTestId('catalog-entry-sew/meet').querySelector('[data-testid="catalog-verified"]')).toBeNull()
    expect(screen.getByTestId('catalog-reason').textContent).toBe('Its first version comes with a K2 update.')
    const meet = screen.getByTestId('catalog-entry-sew/meet')
    expect(meet.querySelector('[data-testid="catalog-state"]')?.textContent).toContain('Not published yet')
    expect((screen.getByTestId('catalog-open-sew/meet') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('catalog-open-sew/hearth') as HTMLButtonElement).disabled).toBe(false)
    expect(screen.queryByTestId('catalog-not-published')).toBeNull()
  })

  it('says plainly that nothing is published yet when no entry has a pinned version', async () => {
    const hearthBlocked = {
      ...HEARTH,
      version: null,
      verified: null,
      canDeploy: false,
      reasons: [{ code: 'catalog_not_pinned', text: 'no version of Hearth is pinned in this K2 yet' }],
    }
    h.entries = [hearthBlocked, MEET_BLOCKED]
    expect(nothingPublished(h.entries as never)).toBe(true)
    expect(nothingPublished([HEARTH, MEET_BLOCKED] as never)).toBe(false)
    expect(nothingPublished([])).toBe(false)
    render(<AppsCatalogSection />)
    const note = await screen.findByTestId('catalog-not-published')
    expect(note.textContent).toContain('aren’t published yet')
    expect(note.textContent).toContain('Hearth and Meet aren’t published yet')
    expect(note.textContent).not.toContain('Sew')
    // Said once, above the card: not again under every row.
    expect(screen.queryAllByTestId('catalog-reason')).toHaveLength(0)
    expect(screen.getAllByText('Not published yet')).toHaveLength(2)
    expect((screen.getByTestId('catalog-open-sew/hearth') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('catalog-open-sew/meet') as HTMLButtonElement).disabled).toBe(true)
    expect(screen.queryByTestId('catalog-sheet')).toBeNull()
    expect(screen.queryAllByTestId('catalog-verified')).toHaveLength(0)
  })

  it('Apps off and an older server each get a sentence, not an error', async () => {
    h.off = true
    render(<AppsCatalogSection />)
    expect((await screen.findByTestId('catalog-apps-off')).textContent).toContain('Apps (beta) are off on this server')
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.queryByTestId('catalog-entry-sew/hearth')).toBeNull()
    cleanup()
    h.off = false
    h.old = true
    render(<AppsCatalogSection />)
    expect((await screen.findByTestId('catalog-server-old')).textContent).toContain('Update K2 on this server')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('prefills the family label, checks it live, offers the suggestion, and deploys for the chosen workspace', async () => {
    h.taken = true
    render(<AppsCatalogSection />)
    fireEvent.click(await screen.findByTestId('catalog-open-sew/hearth'))
    expect((screen.getByTestId('catalog-address') as HTMLInputElement).value).toBe('chat')
    expect(screen.getByRole('heading', { name: 'Deploy Hearth' })).toBeTruthy()
    expect(screen.getByText(/What you approve by deploying/)).toBeTruthy()
    expect(screen.getByText('This App can talk to the chat service on this server, for chat (sew-api)')).toBeTruthy()
    // Opening the card puts the keyboard in the address, and the row's button says it's open.
    expect(document.activeElement).toBe(screen.getByTestId('catalog-address'))
    expect(screen.getByTestId('catalog-open-sew/hearth').getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByTestId('catalog-address-checking')).toBeTruthy()
    const problem = await screen.findByTestId('catalog-address-problem', {}, { timeout: 2000 })
    expect(problem.textContent).toContain('already used by the App hearth. Try hearth-2-chat instead.')
    expect(problem.textContent).not.toContain('k2 app')
    expect((screen.getByTestId('catalog-deploy') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(screen.getByTestId('catalog-use-suggestion'))
    expect((screen.getByTestId('catalog-address') as HTMLInputElement).value).toBe('hearth-2-chat')
    await screen.findByTestId('catalog-address-ok', {}, { timeout: 2000 })
    const check = h.gets.filter((g) => g.route === 'app/catalog/check-address').pop()
    expect(check?.params).toMatchObject({ entry: 'sew/hearth', address: 'hearth-2-chat', workspace: 'ws-sales' })
    fireEvent.click(screen.getByTestId('catalog-deploy'))
    await screen.findByTestId('catalog-job')
    expect(h.posts.find((p) => p.route === 'app/catalog/install')?.body).toEqual({ entry: 'sew/hearth', address: 'hearth-2-chat', workspace: 'ws-sales' })
    expect(screen.getByTestId('catalog-steps').querySelector('[data-step="download"]')?.getAttribute('data-state')).toBe('running')
  })

  it('follows the job, and the owner approves a held request with a vault confirm', async () => {
    h.job = job('waiting', 'approve', { waiting: { reason: 'waiting_approval', requests: ['req-1'], text: 'Waiting for the owner', hint: 'k2 app approve req-1' } })
    h.entries = [{ ...HEARTH, jobs: [h.job] }]
    render(<AppsCatalogSection />)
    const open = await screen.findByTestId('catalog-open-sew/hearth')
    expect(open.textContent).toBe('Show progress')
    fireEvent.click(open)
    await screen.findByTestId('catalog-waiting')
    h.job = job('done', 'ready', {
      next: ['Invite people: k2 app user invite hearth <email>'],
      nextSteps: [{ text: 'Invite people to Hearth from Settings → Apps → Hearth → People.', hint: 'k2 app user invite hearth <email>' }],
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId('catalog-approve'))
    })
    await screen.findByTestId('catalog-done')
    const routes = h.posts.map((p) => p.route)
    expect(routes).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'app/approve'])
    expect(h.posts[2].body).toMatchObject({ request: 'req-1', confirmToken: 'tok-1' })
    expect(screen.getByTestId('catalog-done').textContent).toContain('chat.acme.example.test')
    expect(screen.getByRole('link', { name: 'chat.acme.example.test' }).getAttribute('href')).toBe('https://chat.acme.example.test')
    // What to do next, in words: never the command.
    expect(screen.getByTestId('catalog-done').textContent).toContain('Next: Invite people to Hearth from Settings → Apps → Hearth → People.')
    expect(screen.getByTestId('catalog-done').textContent).not.toContain('k2 app')
  })

  it('a member sees the catalog but no Add button; a failed retryable step offers Retry', async () => {
    h.role = 'member'
    render(<AppsCatalogSection />)
    await screen.findByTestId('catalog-entry-sew/hearth')
    await waitFor(() => expect(screen.getByTestId('catalog-staff-only')).toBeTruthy())
    expect(screen.queryByTestId('catalog-open-sew/hearth')).toBeNull()
    cleanup()
    h.role = 'admin'
    h.job = job('failed', 'download', {
      error: { code: 'download_failed', text: 'Couldn’t download hearth-k2-0.2.4.tar.gz: github.com timed out', hint: 'k2 app catalog retry sj-1', retry: true },
    })
    h.entries = [{ ...HEARTH, jobs: [] }]
    render(<AppsCatalogSection />)
    fireEvent.click(await screen.findByTestId('catalog-open-sew/hearth'))
    await waitFor(() => expect(screen.getByTestId('catalog-deploy')).toBeTruthy())
    await waitFor(() => expect((screen.getByTestId('catalog-deploy') as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(screen.getByTestId('catalog-deploy'))
    await screen.findByTestId('catalog-failed')
    expect(screen.queryByTestId('catalog-approve')).toBeNull()
    const failed = screen.getByTestId('catalog-failed')
    expect(failed.textContent).toContain('The deploy stopped at Download.')
    expect(failed.textContent).toContain('The server said: Couldn’t download hearth-k2-0.2.4.tar.gz: github.com timed out.')
    expect(failed.textContent).not.toContain('download_failed')
    expect(failed.textContent).not.toContain('k2 app')
    expect(screen.getByTestId('catalog-steps').querySelector('[data-step="download"]')?.textContent).toContain('Failed')
    h.job = job('running', 'download')
    fireEvent.click(screen.getByTestId('catalog-retry'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'app/catalog/retry' && p.body.job === 'sj-1')).toBe(true))
  })

  it('a check that cannot run leaves Deploy open; Cancel hands the keyboard back to the row', async () => {
    h.checkFails = true
    render(<AppsCatalogSection />)
    fireEvent.click(await screen.findByTestId('catalog-open-sew/hearth'))
    const unchecked = await screen.findByTestId('catalog-address-unchecked', {}, { timeout: 2000 })
    expect(unchecked.textContent).toContain('Couldn’t check the address')
    expect((screen.getByTestId('catalog-deploy') as HTMLButtonElement).disabled).toBe(false)
    h.checkFails = false
    fireEvent.change(screen.getByTestId('catalog-address'), { target: { value: 'team' } })
    expect(screen.getByTestId('catalog-address-checking')).toBeTruthy()
    expect(screen.queryByTestId('catalog-address-unchecked')).toBeNull()
    await screen.findByTestId('catalog-address-ok', {}, { timeout: 2000 })
    expect(screen.getByTestId('catalog-address-ok').textContent).toContain('team.acme.example.test is free')
    fireEvent.click(screen.getByTestId('catalog-close'))
    expect(screen.queryByTestId('catalog-sheet')).toBeNull()
    expect(document.activeElement).toBe(screen.getByTestId('catalog-open-sew/hearth'))
    expect(screen.getByTestId('catalog-open-sew/hearth').getAttribute('aria-expanded')).toBe('false')
  })

  it('names the state of each step in words, and a wait for rights without its command', async () => {
    h.job = job('waiting', 'address', {
      waiting: { reason: 'waiting_sew_rights', requests: [], text: 'Setting up Hearth needs a permission', hint: 'k2 agent access set <workspace> chat_manage on' },
    })
    h.entries = [{ ...HEARTH, jobs: [h.job] }]
    render(<AppsCatalogSection />)
    fireEvent.click(await screen.findByTestId('catalog-open-sew/hearth'))
    const steps = await screen.findByTestId('catalog-steps')
    expect(steps.querySelector('[data-step="download"]')?.textContent).toContain('Done')
    expect(steps.querySelector('[data-step="address"]')?.textContent).toContain('Waiting')
    expect(steps.querySelector('[data-step="deploy"]')?.textContent).toBe('Deploy')
    const wait = screen.getByTestId('catalog-waiting')
    expect(wait.textContent).not.toContain('k2 agent')
    expect(screen.queryByTestId('catalog-approve')).toBeNull()
  })
})
