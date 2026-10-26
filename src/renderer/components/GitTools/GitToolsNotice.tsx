// First run without git (0.46.4): show the server's git state and give the
// owner one click to install Apple's command line tools. Display + gesture
// only: the daemon detects git, runs `xcode-select --install`, and does the
// waiting git steps itself once the tools land (headless, no client needed).

import React, { useCallback, useEffect, useState } from 'react'
import { Callout } from '@/components/ui'
import {
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

function HowTo({ status }: { status: GitToolsStatus }): React.JSX.Element | null {
  if (status.state === 'ready' || status.howTo.length === 0) return null
  const title = status.canInstall
    ? 'Only reaching this Mac over SSH? Run these yourself (admin password needed):'
    : 'Install git yourself:'
  return (
    <details className="mt-1.5" data-testid="git-tools-howto">
      <summary className="cursor-pointer text-[10px] text-[var(--color-text-muted)]">{title}</summary>
      <pre className="mt-1 whitespace-pre-wrap break-all text-[10px] font-mono text-[var(--color-text-secondary)]">
        {status.howTo.join('\n')}
      </pre>
    </details>
  )
}

function Actions({ h }: { h: GitToolsHandle }): React.JSX.Element | null {
  const s = h.status
  if (!s || s.state === 'ready') return null
  return (
    <div className="flex items-center gap-2 flex-shrink-0">
      {s.canInstall && s.state === 'not_installed' && (
        <button
          type="button"
          data-testid="git-tools-install"
          onClick={() => void h.install()}
          disabled={h.busy}
          className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
        >
          {h.busy ? 'Opening installer…' : 'Install command line tools'}
        </button>
      )}
      <button
        type="button"
        data-testid="git-tools-recheck"
        onClick={() => void h.recheck()}
        disabled={h.busy}
        className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer disabled:opacity-50"
      >
        Check again
      </button>
    </div>
  )
}

function Waiting({ status }: { status: GitToolsStatus }): React.JSX.Element | null {
  if (status.waiting.length === 0) return null
  return (
    <ul className="mt-1 space-y-0.5" data-testid="git-tools-waiting">
      {status.waiting.map((w) => (
        <li key={w.projectId} className="text-[10px] text-[var(--color-text-muted)]">
          <span className="text-[var(--color-text-secondary)]">{w.name}</span> — {w.note}
        </li>
      ))}
    </ul>
  )
}

/** Settings → General line: git state, the install button, waiting workspaces. */
export function GitToolsRow(): React.JSX.Element | null {
  const h = useGitTools()
  const s = h.status
  if (!s) return null
  const dot =
    s.state === 'ready'
      ? 'var(--color-status-ok-soft)'
      : s.state === 'installing'
        ? 'var(--color-accent)'
        : 'var(--color-status-warn-soft)'
  return (
    <div className="py-2 border-b border-[var(--color-border)]" data-settings-id="general.git-tools" data-testid="git-tools-row">
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Git</span>
        <div className="flex items-center gap-3 min-w-0">
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="w-1.5 h-1.5 flex-shrink-0" style={{ backgroundColor: dot }} />
            <span className="text-xs text-[var(--color-text-muted)] truncate" data-testid="git-tools-state">
              {gitToolsStateLabel(s)}
            </span>
          </div>
          <Actions h={h} />
        </div>
      </div>
      {s.state !== 'ready' && s.needs === 'apple_clt' && (
        <p className="mt-1 text-[10px] text-[var(--color-text-muted)]">
          Apple&apos;s command line tools give this Mac git (about 1–2 GB; Xcode isn&apos;t needed). Workspaces
          are added anyway; K2 sets up their git when the tools are ready.
        </p>
      )}
      <Waiting status={s} />
      <HowTo status={s} />
      {h.error && <p className="mt-1 text-[10px] text-[var(--color-status-error-soft)]">{h.error}</p>}
    </div>
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
        {mac
          ? "git isn't installed on this Mac yet. K2 adds the workspace now and sets up git once Apple's command line tools are installed (about 1–2 GB; Xcode isn't needed)."
          : "git isn't installed on this server yet. K2 adds the workspace now and sets up git once git is installed."}
      </p>
      <div className="mt-2 flex items-center justify-between gap-2">
        <span className="text-[10px]" data-testid="git-tools-state">
          {gitToolsStateLabel(s)}
        </span>
        <Actions h={h} />
      </div>
      <HowTo status={s} />
      {h.error && <p className="mt-1 text-[10px]">{h.error}</p>}
    </Callout>
  )
}
