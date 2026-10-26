// @vitest-environment jsdom
// 0.46.4 first run without git: the Settings → General "Git" line and the
// Add-workspace banner only show the daemon's state and send the owner's
// click. The daemon calls are mocked; nothing here installs anything.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { GitToolsStatus } from '@/lib/git-tools'

const fetchGitTools = vi.hoisted(() => vi.fn())
const installGitTools = vi.hoisted(() => vi.fn())
const recheckGitTools = vi.hoisted(() => vi.fn())
vi.mock('@/lib/git-tools', async (orig) => ({
  ...(await orig<typeof import('@/lib/git-tools')>()),
  fetchGitTools,
  installGitTools,
  recheckGitTools,
}))

import { GIT_TOOLS_POLL_MS, GitToolsBanner, GitToolsRow } from './GitToolsNotice'

const MAC_HOW_TO = [
  'touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress',
  'softwareupdate --list   # note the "Command Line Tools for Xcode-NN.N" label',
  'sudo softwareupdate -i "Command Line Tools for Xcode-NN.N" --verbose',
  'rm /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress',
]

function status(over: Partial<GitToolsStatus>): GitToolsStatus {
  return {
    os: 'macos',
    state: 'not_installed',
    needs: 'apple_clt',
    gitPath: null,
    gitVersion: null,
    canInstall: true,
    installStartedAt: null,
    howTo: MAC_HOW_TO,
    waiting: [],
    ...over,
  }
}

const READY = status({
  state: 'ready',
  needs: null,
  gitPath: '/usr/bin/git',
  gitVersion: 'git version 2.39.5 (Apple Git-154)',
  canInstall: false,
  howTo: [],
})

/** Let the mount fetch resolve. */
async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

beforeEach(() => {
  fetchGitTools.mockReset()
  installGitTools.mockReset()
  recheckGitTools.mockReset()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('GitToolsRow (Settings → General)', () => {
  it('offers the one-click install on a Mac without the tools, then shows installing', async () => {
    fetchGitTools.mockResolvedValue(
      status({ waiting: [{ projectId: 'p1', name: 'site', path: '/Users/me/site', branch: 'main', note: "git: waiting for Apple's command line tools" }] }),
    )
    installGitTools.mockResolvedValue(status({ state: 'installing', started: 'started', installStartedAt: 1 }))
    render(<GitToolsRow />)
    await settle()
    expect(screen.getByTestId('git-tools-state').textContent).toBe("Not installed (needs Apple's command line tools)")
    expect(screen.getByTestId('git-tools-waiting').textContent).toBe("site — git: waiting for Apple's command line tools")
    expect(screen.getByTestId('git-tools-howto').textContent).toContain('sudo softwareupdate -i')

    fireEvent.click(screen.getByTestId('git-tools-install'))
    await settle()
    expect(installGitTools).toHaveBeenCalledTimes(1)
    expect(screen.getByTestId('git-tools-state').textContent).toBe("Installing Apple's command line tools…")
    expect(screen.queryByTestId('git-tools-install')).toBeNull()
  })

  it('shows Ready with the version and no buttons once git works', async () => {
    fetchGitTools.mockResolvedValue(READY)
    render(<GitToolsRow />)
    await settle()
    expect(screen.getByTestId('git-tools-state').textContent).toBe('Ready (git version 2.39.5 (Apple Git-154))')
    expect(screen.queryByTestId('git-tools-install')).toBeNull()
    expect(screen.queryByTestId('git-tools-recheck')).toBeNull()
    expect(screen.queryByTestId('git-tools-howto')).toBeNull()
  })

  it('renders nothing when the server has no git-tools route (older server)', async () => {
    fetchGitTools.mockRejectedValue(new Error('404 Not Found'))
    const { container } = render(<GitToolsRow />)
    await settle()
    expect(container.innerHTML).toBe('')
  })

  it('asks the daemon again while git is missing and stops once it is ready', async () => {
    vi.useFakeTimers()
    fetchGitTools.mockResolvedValueOnce(status({ state: 'installing' })).mockResolvedValue(READY)
    render(<GitToolsRow />)
    await settle()
    expect(fetchGitTools).toHaveBeenCalledTimes(1)
    await act(async () => {
      vi.advanceTimersByTime(GIT_TOOLS_POLL_MS)
    })
    await settle()
    expect(fetchGitTools).toHaveBeenCalledTimes(2)
    expect(screen.getByTestId('git-tools-state').textContent).toContain('Ready')
    await act(async () => {
      vi.advanceTimersByTime(GIT_TOOLS_POLL_MS * 3)
    })
    expect(fetchGitTools).toHaveBeenCalledTimes(2)
  })

  it('Check again posts a recheck and shows its error plainly', async () => {
    fetchGitTools.mockResolvedValue(status({}))
    recheckGitTools.mockRejectedValue(new Error('daemon unreachable'))
    render(<GitToolsRow />)
    await settle()
    fireEvent.click(screen.getByTestId('git-tools-recheck'))
    await settle()
    expect(recheckGitTools).toHaveBeenCalledTimes(1)
    expect(screen.getByText('daemon unreachable')).toBeTruthy()
  })
})

describe('GitToolsBanner (Add workspace)', () => {
  it('is empty while git works', async () => {
    fetchGitTools.mockResolvedValue(READY)
    const { container } = render(<GitToolsBanner />)
    await settle()
    expect(container.innerHTML).toBe('')
  })

  it('on a Mac without the tools: says the workspace is added anyway and offers the install', async () => {
    fetchGitTools.mockResolvedValue(status({}))
    render(<GitToolsBanner />)
    await settle()
    const banner = screen.getByTestId('git-tools-banner')
    expect(banner.textContent).toContain("git isn't installed on this Mac yet. K2 adds the workspace now")
    expect(banner.textContent).toContain("Xcode isn't needed")
    expect(screen.getByTestId('git-tools-install')).toBeTruthy()
  })

  it('on Linux: no install button, the apt line to run yourself', async () => {
    fetchGitTools.mockResolvedValue(
      status({ os: 'linux', needs: 'git', canInstall: false, howTo: ['sudo apt install git'] }),
    )
    render(<GitToolsBanner />)
    await settle()
    expect(screen.getByTestId('git-tools-banner').textContent).toContain("git isn't installed on this server yet")
    expect(screen.queryByTestId('git-tools-install')).toBeNull()
    expect(screen.getByTestId('git-tools-howto').textContent).toContain('Install git yourself:')
    expect(screen.getByTestId('git-tools-howto').textContent).toContain('sudo apt install git')
  })
})
