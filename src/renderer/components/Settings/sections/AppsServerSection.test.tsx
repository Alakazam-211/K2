// @vitest-environment jsdom

// Settings → Apps → This server (0.46.3, "Apps on for real"): the owner sets
// up Apps on the window's server from the desktop app. The Apps (beta)
// switch is the owner's (others see it disabled, with the reason); the box
// doctor shows each check with its fix; Prepare runs a dry run FIRST and
// lists every change, and only an explicit Apply changes the box (with K2's
// Podman when the dry run says so); Install App engine posts the engine
// route; the per-agent switches write the same agent-access toggles as the
// Agent tab. Every call goes to the window's server (primaryScope).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  settingsUpdate: vi.fn(),
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
    daemonCliPostQuery: vi.fn(),
  }
})
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: (...a: unknown[]) => h.settingsUpdate(...a),
  settingsReset: vi.fn(async () => ({})),
}))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn() }))

import * as presence from '@/components/Presence/PresenceKickButton'
import { useSettingsStore, appsSwitchFrom } from '@/stores/settings'
import { AppsServerSection, APPS_SERVER_MANIFEST } from './AppsServerSection'
import { changeLines, needsPodmanInstall, parseAutoPrepare, parseDoctor, parseDryRun, parseUnprepResult } from './apps-server-api'

const role = vi.mocked(presence.fetchViewerRole)

const APPS_OFF = new Error(
  JSON.stringify({ error: { code: 'apps_disabled', hint: 'Apps (beta) are off on this server' } }),
)

function doctor(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ready: false,
    apps: true,
    processEngine: false,
    linux: true,
    helperProtocol: 8,
    appsMinProtocol: 6,
    runtime: 'crun',
    checks: [
      { id: 'platform', status: 'ok', detail: 'Linux', fix: null },
      { id: 'podman', status: 'fail', detail: 'Podman 4.9.3: Apps need 5+', fix: 'k2 app box prepare --install-podman (owner; --dry-run first to see every change)' },
      { id: 'appUser', status: 'fail', detail: 'no k2app user', fix: 'k2 app box prepare (owner)' },
      { id: 'engine', status: 'warn', detail: 'k2-app-engine not installed yet', fix: 'k2 app engine install (owner)' },
      { id: 'processEngine', status: 'ok', detail: 'the unwalled process engine is off', fix: null },
    ],
    ...over,
  }
}

const DRY = {
  ok: true,
  dryRun: true,
  changed: false,
  podman: {
    action: 'offer',
    have: '4.9.3',
    os: 'Ubuntu 24.04',
    bundle: { version: '5.4.2', source: 'k2', url: 'https://example.invalid/podman.tar.zst', sha256: 'abc123', size: 10, writes: ['/opt/k2/podman'] },
  },
  steps: ['apps podman install', 'apps packages install', 'apps ensure-user'],
  box: doctor(),
}

/** GET routes by name; anything unmapped fails the test. */
function gets(map: Record<string, () => unknown>): void {
  h.daemonCliGet.mockImplementation(async (route: string) => {
    const f = map[route]
    if (!f) throw new Error(`unexpected GET ${route}`)
    return f()
  })
}

/** POST routes by name; anything unmapped fails the test. */
function posts(map: Record<string, (body: unknown) => unknown>): void {
  h.daemonCliPost.mockImplementation(async (route: string, body: unknown) => {
    const f = map[route]
    if (!f) throw new Error(`unexpected POST ${route}`)
    return f(body)
  })
}

const postsTo = (route: string): unknown[] =>
  h.daemonCliPost.mock.calls.filter((c) => c[0] === route).map((c) => c[1])

beforeEach(() => {
  vi.clearAllMocks()
  role.mockResolvedValue('owner')
  useSettingsStore.setState({ apps: true } as never)
})

afterEach(() => cleanup())

describe('the Apps (beta) switch', () => {
  it('is the owner’s: on, enabled, and a flip writes `apps` to the window’s server', async () => {
    gets({ 'app/box': () => doctor() })
    h.settingsUpdate.mockResolvedValue({ apps: false })
    render(<AppsServerSection />)
    const sw = await screen.findByRole('switch', { name: 'Apps (beta)' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('true')
    gets({ 'app/box': () => { throw APPS_OFF } })
    fireEvent.click(sw)
    await waitFor(() => expect(h.settingsUpdate).toHaveBeenCalledWith({ apps: false }))
    await screen.findByTestId('apps-off')
    expect(useSettingsStore.getState().apps).toBe(false)
  })

  it('is disabled with the reason for anyone but the owner', async () => {
    role.mockResolvedValue('admin')
    gets({ 'app/box': () => doctor() })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-switch-owner-only')
    const sw = screen.getByRole('switch', { name: 'Apps (beta)' }) as HTMLButtonElement
    expect(sw.disabled).toBe(true)
    expect((screen.getByTestId('apps-engine-install') as HTMLButtonElement).disabled).toBe(true)
  })

  it('a refused flip rolls back and shows the daemon’s words', async () => {
    gets({ 'app/box': () => doctor() })
    h.settingsUpdate.mockRejectedValue(new Error('settings_update 403: {"error":"updating \\"apps\\" requires the Owner role"}'))
    render(<AppsServerSection />)
    const sw = await screen.findByRole('switch', { name: 'Apps (beta)' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    const notice = await screen.findByTestId('apps-server-notice')
    expect(notice.textContent).toMatch(/requires the Owner role/)
    expect(useSettingsStore.getState().apps).toBe(true)
  })

  it('with Apps off the doctor isn’t shown, and turning them on loads it', async () => {
    useSettingsStore.setState({ apps: false } as never)
    gets({ 'app/box': () => { throw APPS_OFF } })
    h.settingsUpdate.mockResolvedValue({ apps: true })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-off')
    expect(screen.queryByTestId('apps-doctor')).toBeNull()
    const sw = screen.getByRole('switch', { name: 'Apps (beta)' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    gets({ 'app/box': () => doctor() })
    fireEvent.click(sw)
    await screen.findByTestId('apps-doctor')
    expect(h.settingsUpdate).toHaveBeenCalledWith({ apps: true })
  })
})

describe('the box doctor', () => {
  it('shows each check with its status and fix', async () => {
    gets({ 'app/box': () => doctor() })
    render(<AppsServerSection />)
    const podman = await screen.findByTestId('apps-check-podman')
    expect(podman.getAttribute('data-status')).toBe('fail')
    expect(podman.textContent).toMatch(/Podman 4\.9\.3/)
    expect(podman.textContent).toMatch(/Fix: k2 app box prepare --install-podman/)
    expect(screen.getByTestId('apps-check-platform').textContent).not.toMatch(/Fix:/)
    expect(screen.getByTestId('apps-check-processEngine').getAttribute('data-status')).toBe('ok')
    expect(screen.getByTestId('apps-doctor').textContent).toMatch(/Not ready yet/)
  })

  it('off Linux there is nothing to prepare', async () => {
    gets({ 'app/box': () => doctor({ linux: false, checks: [{ id: 'platform', status: 'fail', detail: 'Apps run on Linux boxes only', fix: null }] }) })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-doctor')
    expect(screen.queryByTestId('apps-setup')).toBeNull()
    expect(screen.getByTestId('apps-doctor').textContent).toMatch(/Linux servers/)
  })
})

describe('Prepare this box', () => {
  it('runs a dry run first, lists every change, and applies only on Apply (with K2’s Podman)', async () => {
    gets({ 'app/box': () => doctor() })
    posts({
      'app/box/prepare': (b) => ((b as { dryRun?: boolean }).dryRun ? DRY : { ok: true, box: doctor({ ready: true }) }),
    })
    render(<AppsServerSection />)
    fireEvent.click(await screen.findByTestId('apps-prepare-dry-run'))
    const list = await screen.findByTestId('apps-dry-run')
    expect(postsTo('app/box/prepare')).toEqual([{ dryRun: true }])
    expect(list.textContent).toMatch(/Install K2's Podman 5\.4\.2/)
    expect(list.textContent).toMatch(/apps ensure-user/)
    expect(list.textContent).toMatch(/abc123/)
    const apply = screen.getByTestId('apps-prepare-apply') as HTMLButtonElement
    expect(apply.textContent).toMatch(/installs Podman/)
    fireEvent.click(apply)
    await screen.findByText('The box is prepared.')
    expect(postsTo('app/box/prepare')).toEqual([{ dryRun: true }, { installPodman: true }])
  })

  it('a box that needs no Podman applies without installPodman', async () => {
    gets({ 'app/box': () => doctor() })
    posts({
      'app/box/prepare': (b) =>
        (b as { dryRun?: boolean }).dryRun
          ? { ...DRY, podman: { action: 'keep', have: '5.4.2', source: 'distro' }, steps: ['apps packages install'] }
          : { ok: true, box: doctor({ ready: true }) },
    })
    render(<AppsServerSection />)
    fireEvent.click(await screen.findByTestId('apps-prepare-dry-run'))
    await screen.findByTestId('apps-dry-run')
    fireEvent.click(screen.getByTestId('apps-prepare-apply'))
    await screen.findByText('The box is prepared.')
    expect(postsTo('app/box/prepare')).toEqual([{ dryRun: true }, {}])
  })

  it('a failed apply shows the daemon’s words and re-reads the box', async () => {
    gets({ 'app/box': () => doctor() })
    posts({
      'app/box/prepare': (b) => {
        if ((b as { dryRun?: boolean }).dryRun) return DRY
        throw new Error(JSON.stringify({ error: { code: 'box_prep_failed', hint: 'box prep stopped at `apps subid`' } }))
      },
    })
    render(<AppsServerSection />)
    fireEvent.click(await screen.findByTestId('apps-prepare-dry-run'))
    await screen.findByTestId('apps-dry-run')
    const before = h.daemonCliGet.mock.calls.length
    fireEvent.click(screen.getByTestId('apps-prepare-apply'))
    const notice = await screen.findByTestId('apps-server-notice')
    expect(notice.textContent).toMatch(/stopped at `apps subid`/)
    await waitFor(() => expect(h.daemonCliGet.mock.calls.length).toBeGreaterThan(before))
  })

  it('Apply is the owner’s; anyone may look at the dry run', async () => {
    role.mockResolvedValue('admin')
    gets({ 'app/box': () => doctor() })
    posts({ 'app/box/prepare': () => DRY })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-switch-owner-only')
    fireEvent.click(screen.getByTestId('apps-prepare-dry-run'))
    await screen.findByTestId('apps-dry-run')
    expect((screen.getByTestId('apps-prepare-apply') as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('Install App engine', () => {
  it('posts the engine route and says what answered', async () => {
    gets({ 'app/box': () => doctor() })
    posts({ 'app/engine/install': () => ({ ok: true, engine: { version: '0.46.3', protocol: 3 } }) })
    render(<AppsServerSection />)
    const b = await screen.findByTestId('apps-engine-install')
    await waitFor(() => expect((b as HTMLButtonElement).disabled).toBe(false))
    expect(b.textContent).toBe('Install App engine')
    fireEvent.click(b)
    const notice = await screen.findByTestId('apps-server-notice')
    expect(notice.textContent).toMatch(/0\.46\.3 is installed and answering/)
    expect(postsTo('app/engine/install')).toEqual([{}])
  })
})

describe('automatic prep stopped', () => {
  const blocked = {
    enabled: true,
    default: false,
    setting: 'appsAutoPrepare',
    lastRun: 1,
    outcome: { state: 'blocked', code: 'kernel_too_old', why: 'Linux 5.4: Apps need 5.11 or newer' },
    nextAttempt: null,
    blocked: true,
    undoneAt: null,
    retryRoute: '/cli/app/box/auto-prepare/retry',
  }

  it('shows why it stopped, and Retry posts the retry route', async () => {
    gets({ 'app/box': () => doctor({ autoPrepare: blocked }) })
    posts({ 'app/box/auto-prepare/retry': () => ({ ok: true, autoPrepare: { ...blocked, blocked: false, outcome: null } }) })
    render(<AppsServerSection />)
    const stopped = await screen.findByTestId('apps-auto-prepare-stopped')
    expect(stopped.textContent).toMatch(/Stopped: Linux 5\.4: Apps need 5\.11 or newer/)
    expect(stopped.textContent).toMatch(/until you press Retry or K2 updates/)
    const retry = screen.getByTestId('apps-auto-prepare-retry') as HTMLButtonElement
    await waitFor(() => expect(retry.disabled).toBe(false))
    fireEvent.click(retry)
    await screen.findByText('Automatic prep will run again now.')
    expect(postsTo('app/box/auto-prepare/retry')).toEqual([{}])
  })

  it('an undone box says it is paused; Retry is the owner’s', async () => {
    role.mockResolvedValue('admin')
    gets({ 'app/box': () => doctor({ autoPrepare: { ...blocked, blocked: false, outcome: { state: 'ready' }, undoneAt: 5 } }) })
    render(<AppsServerSection />)
    const stopped = await screen.findByTestId('apps-auto-prepare-stopped')
    expect(stopped.textContent).toMatch(/Paused because you undid the box setup/)
    await screen.findByTestId('apps-switch-owner-only')
    expect((screen.getByTestId('apps-auto-prepare-retry') as HTMLButtonElement).disabled).toBe(true)
  })

  it('a prep that is not stopped shows no Retry', async () => {
    gets({ 'app/box': () => doctor({ autoPrepare: { ...blocked, blocked: false, outcome: { state: 'ready' } } }) })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-auto-prepare-last')
    expect(screen.queryByTestId('apps-auto-prepare-retry')).toBeNull()
  })
})

describe('Undo box setup', () => {
  const PLAN = {
    ok: true,
    dryRun: true,
    changed: false,
    undoes: ['nft remove', 'apparmor off', 'linger off'],
    kept: ['the k2app user and its subordinate ids', 'distro packages'],
    runningApps: [] as string[],
  }

  it('lists the plan first and undoes only on the second, explicit click', async () => {
    gets({ 'app/box': () => doctor() })
    posts({
      'app/box/unprep': (b) =>
        (b as { dryRun?: boolean }).dryRun
          ? PLAN
          : {
              ok: true,
              result: { unprepared: true, steps: [{ step: 'nft remove', changed: true }], kept: PLAN.kept },
              kept: PLAN.kept,
              note: "K2's Podman stays: k2 app box podman uninstall removes it.",
              box: doctor(),
            },
    })
    render(<AppsServerSection />)
    const start = await screen.findByTestId('apps-unprep-start')
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(start)
    const plan = await screen.findByTestId('apps-unprep-plan')
    expect(plan.textContent).toMatch(/nft remove/)
    expect(plan.textContent).toMatch(/Kept: the k2app user/)
    expect(postsTo('app/box/unprep')).toEqual([{ dryRun: true }])
    fireEvent.click(screen.getByTestId('apps-unprep-confirm'))
    const notice = await screen.findByTestId('apps-server-notice')
    expect(notice.textContent).toMatch(/The box setup is undone\. K2's Podman stays/)
    expect(postsTo('app/box/unprep')).toEqual([{ dryRun: true }, { confirm: true }])
  })

  it('Cancel posts nothing more', async () => {
    gets({ 'app/box': () => doctor() })
    posts({ 'app/box/unprep': () => PLAN })
    render(<AppsServerSection />)
    const start = await screen.findByTestId('apps-unprep-start')
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(start)
    const plan = await screen.findByTestId('apps-unprep-plan')
    fireEvent.click(within(plan).getByText('Cancel'))
    expect(screen.queryByTestId('apps-unprep-plan')).toBeNull()
    expect(postsTo('app/box/unprep')).toEqual([{ dryRun: true }])
  })

  it('running Apps disable the confirm and are named', async () => {
    gets({ 'app/box': () => doctor() })
    posts({ 'app/box/unprep': () => ({ ...PLAN, runningApps: ['shop', 'blog'] }) })
    render(<AppsServerSection />)
    const start = await screen.findByTestId('apps-unprep-start')
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(start)
    const running = await screen.findByTestId('apps-unprep-running')
    expect(running.textContent).toMatch(/Stop these Apps first: shop, blog/)
    expect((screen.getByTestId('apps-unprep-confirm') as HTMLButtonElement).disabled).toBe(true)
    expect(postsTo('app/box/unprep')).toEqual([{ dryRun: true }])
  })

  it('is the owner’s', async () => {
    role.mockResolvedValue('admin')
    gets({ 'app/box': () => doctor() })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-switch-owner-only')
    expect((screen.getByTestId('apps-unprep-start') as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('independent of the per-agent switches', () => {
  it('shows no agent switch of its own (they stay on the Agent tab)', async () => {
    gets({ 'app/box': () => doctor() })
    render(<AppsServerSection />)
    await screen.findByTestId('apps-doctor')
    expect(screen.getAllByRole('switch').map((s) => s.getAttribute('aria-label'))).toEqual(['Apps (beta)'])
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })
})

describe('helpers', () => {
  it('reads an older daemon’s answers', () => {
    expect(appsSwitchFrom({ apps: false, appsPreview: true })).toBe(false)
    expect(appsSwitchFrom({ appsPreview: true })).toBe(true)
    expect(appsSwitchFrom({})).toBe(false)
    expect(parseDoctor({ preview: true, checks: [] }).apps).toBe(true)
  })

  it('lists the Podman install first, then the steps', () => {
    const d = parseDryRun(DRY)
    expect(needsPodmanInstall(d)).toBe(true)
    expect(changeLines(d)[0]).toMatch(/Install K2's Podman 5\.4\.2/)
    expect(changeLines(d)).toContain('  writes /opt/k2/podman')
    expect(changeLines(d).slice(-3)).toEqual(DRY.steps)
    expect(needsPodmanInstall(parseDryRun({ podman: { action: 'keep' } }))).toBe(false)
  })

  it('reads the auto-prep stop and the undo result', () => {
    expect(parseAutoPrepare({ enabled: true, outcome: { state: 'blocked', code: 'cgroup_v1', why: 'no cgroup v2' } })).toEqual({
      enabled: true,
      state: 'blocked',
      detail: 'no cgroup v2',
      blocked: true,
      code: 'cgroup_v1',
      undoneAt: null,
    })
    expect(parseAutoPrepare({ enabled: false, undoneAt: 7 })?.undoneAt).toBe(7)
    const r = parseUnprepResult({ result: { steps: [{ step: 'nft remove', changed: true }], kept: ['x'] }, note: 'n' })
    expect(r).toEqual({ steps: [{ step: 'nft remove', changed: true }], kept: ['x'], note: 'n', box: null })
  })

  it('the manifest points at the Apps section', () => {
    expect(APPS_SERVER_MANIFEST.every((e) => e.section === 'skin-access')).toBe(true)
  })
})
