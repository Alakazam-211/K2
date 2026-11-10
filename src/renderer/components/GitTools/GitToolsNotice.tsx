// First run without git (0.46.4): show the server's git state and give the
// owner one click to install Apple's command line tools. Display + gesture
// only: the daemon detects git, runs `xcode-select --install`, and does the
// waiting git steps itself once the tools land (headless, no client needed).

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import { SettingRow } from '@/components/Settings/controls/SettingControls'
import { StatusDot, type HealthTone } from '@/components/Settings/sections/ConnectionsParts'
import {
  APPLE_INSTALLER_HINT,
  fetchGitTools,
  gitToolsStateLabel,
  installGitTools,
  recheckGitTools,
  type GitToolsStatus,
} from '@/lib/git-tools'

/** While git isn't ready, ask the daemon again this often. */
export const GIT_TOOLS_POLL_MS = 5000

export interface GitToolsHandle {
  status: GitToolsStatus | null
  busy: boolean
  error: string | null
  install: () => Promise<void>
  recheck: () => Promise<void>
}

export function useGitTools(): GitToolsHandle {
  const [status, setStatus] = useState<GitToolsStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      setStatus(await fetchGitTools())
    } catch {
      // An older server has no /cli/git-tools: show nothing.
      setStatus(null)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const ready = status?.state === 'ready'
  useEffect(() => {
    if (!status || ready) return
    const t = setInterval(() => void load(), GIT_TOOLS_POLL_MS)
    return () => clearInterval(t)
  }, [status, ready, load])

  const run = useCallback(async (fn: () => Promise<GitToolsStatus>) => {
    setBusy(true)
    setError(null)
    try {
      setStatus(await fn())
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [])

  return {
    status,
    busy,
    error,
    install: useCallback(() => run(installGitTools), [run]),
    recheck: useCallback(() => run(recheckGitTools), [run]),
  }
}

/** A git that is there but fails (Xcode's licence not accepted, …). */
function Broken({ status }: { status: GitToolsStatus }): React.JSX.Element | null {
  if (status.state !== 'git_broken') return null
  return (
    <div className="mt-1 type-hint" data-testid="git-tools-broken">
      <p className="break-all">
        {status.gitPath}: {status.gitError}
      </p>
      {status.howTo.map((line) => (
        <p key={line} className="mt-0.5 text-[var(--color-text-primary)]">
          {line}
        </p>
      ))}
    </div>
  )
}

function HowTo({ status }: { status: GitToolsStatus }): React.JSX.Element | null {
  if (status.state === 'ready' || status.state === 'git_broken' || status.howTo.length === 0) return null
  const title = status.canInstall
    ? 'Only reaching this Mac over SSH? Run these yourself (admin password needed):'
    : 'Install git yourself:'
  return (
    <details className="mt-1.5" data-testid="git-tools-howto">
      <summary className="outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-[var(--color-accent)] cursor-pointer type-hint text-[var(--color-text-secondary)]">{title}</summary>
      <pre className="mt-1 whitespace-pre-wrap break-all type-hint font-mono text-[var(--color-text-secondary)]">
        {status.howTo.join('\n')}
      </pre>
    </details>
  )
}

function Actions({ h, primary = true }: { h: GitToolsHandle; primary?: boolean }): React.JSX.Element | null {
  const s = h.status
  if (!s || s.state === 'ready') return null
  return (
    <div className="flex items-center gap-2 flex-shrink-0">
      {s.canInstall && s.state === 'not_installed' && (
        <Button
          variant={primary ? 'accent' : 'default'}
          data-testid="git-tools-install"
          onClick={() => void h.install()}
          disabled={h.busy}
        >
          {h.busy ? 'Opening installer…' : 'Install command line tools'}
        </Button>
      )}
      <Button variant="quiet" data-testid="git-tools-recheck" onClick={() => void h.recheck()} disabled={h.busy}>
        Check again
      </Button>
    </div>
  )
}

/** While Apple's installer runs: what its dialog wants from the person. */
function InstallerHint({ status }: { status: GitToolsStatus }): React.JSX.Element | null {
  if (status.state !== 'installing') return null
  return (
    <p className="mt-1 type-hint" data-testid="git-tools-installer-hint">
      {APPLE_INSTALLER_HINT}
    </p>
  )
}

function Waiting({ status }: { status: GitToolsStatus }): React.JSX.Element | null {
  if (status.waiting.length === 0) return null
  return (
    <ul className="mt-1 space-y-0.5" data-testid="git-tools-waiting">
      {status.waiting.map((w) => (
        <li key={w.projectId} className="type-hint text-[var(--color-text-secondary)]">
          <span className="text-[var(--color-text-secondary)]">{w.name}</span> — {w.note}
          {w.lastError && (
            <span className="block text-[var(--color-status-error-soft)]" data-testid="git-tools-waiting-error">
              Last try: {w.lastError}
            </span>
          )}
        </li>
      ))}
    </ul>
  )
}

/** Settings → General row: git state, the install button, waiting workspaces.
 *  A row of General's card (SettingRow), so it lines up with the rest. */
export function GitToolsRow({ primary = true }: { primary?: boolean } = {}): React.JSX.Element | null {
  const h = useGitTools()
  const s = h.status
  if (!s) return null
  const tone: HealthTone = s.state === 'ready' ? 'ok' : s.state === 'installing' ? 'neutral' : 'warn'
  const notes =
    s.state !== 'ready' ? (
      <div className="space-y-1">
        {s.needs === 'apple_clt' && (
          <p className="type-hint text-[var(--color-text-secondary)]">
            Apple&apos;s command line tools give this Mac git (about 1–2 GB; Xcode isn&apos;t needed). Workspaces
            are added anyway; K2 sets up their git when the tools are ready.
          </p>
        )}
        <div className="text-[var(--color-text-secondary)]">
          <InstallerHint status={s} />
        </div>
        <div className="text-[var(--color-text-secondary)]">
          <Broken status={s} />
        </div>
        <Waiting status={s} />
        <HowTo status={s} />
      </div>
    ) : null
  return (
    <SettingRow
      label="Git"
      settingId="general.git-tools"
      testId="git-tools-row"
      hint={
        <span className="inline-flex items-center gap-1.5 min-w-0">
          <StatusDot tone={tone} />
          <span data-testid="git-tools-state">{gitToolsStateLabel(s)}</span>
        </span>
      }
      below={
        notes || h.error ? (
          <>
            {notes}
            {h.error && (
              <Callout tone="error" role="alert" className="mt-2">
                <p className="type-hint">{h.error}</p>
              </Callout>
            )}
          </>
        ) : undefined
      }
    >
      <Actions h={h} primary={primary} />
    </SettingRow>
  )
}

/** Add-workspace / onboarding banner: nothing while git works. */
export function GitToolsBanner(): React.JSX.Element | null {
  const h = useGitTools()
  const s = h.status
  if (!s || s.state === 'ready') return null
  const mac = s.needs === 'apple_clt'
  return (
    <Callout tone="warn" data-testid="git-tools-banner">
      <p>
        {s.state === 'git_broken'
          ? "git is installed on this server but fails. K2 adds the workspace now and sets up git once it works."
          : mac
            ? "git isn't installed on this Mac yet. K2 adds the workspace now and sets up git once Apple's command line tools are installed (about 1–2 GB; Xcode isn't needed)."
            : "git isn't installed on this server yet. K2 adds the workspace now and sets up git once git is installed."}
      </p>
      <div className="mt-2 flex items-center justify-between gap-2">
        <span className="type-hint" data-testid="git-tools-state">
          {gitToolsStateLabel(s)}
        </span>
        <Actions h={h} />
      </div>
      <InstallerHint status={s} />
      <Broken status={s} />
      <HowTo status={s} />
      {h.error && <p className="mt-1 type-hint">{h.error}</p>}
    </Callout>
  )
}
