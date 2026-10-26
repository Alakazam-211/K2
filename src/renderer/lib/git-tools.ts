// First run without git (0.46.4). The daemon owns the truth: whether git
// runs on the server, the state of Apple's command line tools install, and
// the workspaces whose git steps wait (`GET /cli/git-tools`). The client
// only shows it and sends the owner's click (`POST /cli/git-tools/install`,
// which opens Apple's installer on that Mac; never sudo).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type GitToolsState = 'ready' | 'not_installed' | 'installing' | 'git_broken'

export interface GitWaitingWorkspace {
  projectId: string
  name: string
  path: string
  branch: string
  /** "git: waiting for Apple's command line tools" */
  note: string
  /** The last failed try of the waiting git steps, if any. */
  lastError: string | null
}

export interface GitToolsStatus {
  os: string
  state: GitToolsState
  needs: 'apple_clt' | 'git' | 'fix_git' | null
  gitPath: string | null
  gitVersion: string | null
  /** state `git_broken`: what the git that is there said. */
  gitError: string | null
  /** macOS and git not running: the one-click install is offered. */
  canInstall: boolean
  installStartedAt: number | null
  /** Commands the owner runs themselves (Mac over SSH, Linux). */
  howTo: string[]
  waiting: GitWaitingWorkspace[]
  /** install only: `started` | `already_ready`. */
  started?: string
  /** recheck only: folders whose waiting git steps just finished. */
  finished?: string[]
}

export function fetchGitTools(): Promise<GitToolsStatus> {
  return daemonCliGet<GitToolsStatus>(primaryScope(), 'git-tools')
}

export function installGitTools(): Promise<GitToolsStatus> {
  return daemonCliPost<GitToolsStatus>(primaryScope(), 'git-tools/install', {})
}

export function recheckGitTools(): Promise<GitToolsStatus> {
  return daemonCliPost<GitToolsStatus>(primaryScope(), 'git-tools/recheck', {})
}

/** What Apple's dialog wants, shown while the install runs. */
export const APPLE_INSTALLER_HINT =
  "Apple's installer is open (it can be behind the K2 window). Click Install, not Get Xcode (that opens the App Store for the full Xcode), agree to Apple's licence, then wait for the download (about 5–15 min)."

/** One short line for the state. */
export function gitToolsStateLabel(s: GitToolsStatus): string {
  if (s.state === 'ready') return s.gitVersion ? `Ready (${s.gitVersion})` : 'Ready'
  if (s.state === 'installing') return "Installing Apple's command line tools…"
  if (s.state === 'git_broken') return 'git is installed but fails'
  return s.needs === 'apple_clt' ? "Not installed (needs Apple's command line tools)" : 'git is not installed'
}
