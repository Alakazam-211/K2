// An agent's clone proposals, on top of its Thread (0.46.1).
//
// `k2 clone propose` posts a plain line on the workspace's main Thread
// ("Proposed: … Review in K2 or k2 clone approve <id>"). This banner is the
// "Review in K2": for the Owner and Admins it lists the workspace's open
// proposals (daemon truth, `GET /cli/clone-proposal/list`), and
// "Review clone…" opens the Clone dialog prefilled from the proposal, where
// one confirm approves and runs it. "Deny" tells the agent no. Hidden for
// Members, for addresses that aren't a workspace (`import:`, `gb:`), and
// when there's nothing open. Refetches when the Thread grows (the proposal
// line arrives) via `refreshKey`.

import React, { useCallback, useEffect, useState } from 'react'

import { Button } from '@/components/ui'
import { useRoom } from '@/components/Room/RoomContext'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { denyProposal, fetchOpenProposals, proposerName, type CloneProposal } from '@/lib/clone-proposals'

/** The workspace part of a Thread address, or null for a non-workspace one. */
export function proposalWorkspaceOf(addr: string): string | null {
  const a = addr.trim()
  if (!a || a.startsWith('import:') || a.startsWith('gb:')) return null
  return a.split('/')[0] || null
}

/** The proposal's target among this window's Connect hosts (`luzz.k2.dev`
 *  or `host:port`), or null. */
export function hostForTarget(hosts: readonly ConnectHost[], target: string): ConnectHost | null {
  const t = target.trim().toLowerCase()
  if (!t || t === 'here') return null
  return (
    hosts.find((h) => {
      const name = h.hostname.toLowerCase()
      return name === t || `${name}:${h.port}` === t
    }) ?? null
  )
}

function whoCanApprove(w: { owner?: unknown; role?: unknown } | null): boolean {
  return !!w && (w.owner === true || w.role === 'owner' || w.role === 'admin')
}

export function CloneProposalsBanner({ addr, refreshKey }: { addr: string; refreshKey?: number }): React.JSX.Element | null {
  const room = useRoom()
  const workspace = proposalWorkspaceOf(addr)
  const hosts = useConnectHostStore((s) => s.hosts)
  const [canApprove, setCanApprove] = useState(false)
  const [rows, setRows] = useState<CloneProposal[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  useEffect(() => {
    if (!workspace) return
    let cancelled = false
    daemonCliGet<{ owner?: unknown; role?: unknown }>(room.scope, 'auth/whoami').then(
      (w) => {
        if (!cancelled) setCanApprove(whoCanApprove(w))
      },
      (e: unknown) => console.warn('[clone-proposals] whoami:', e),
    )
    return () => {
      cancelled = true
    }
  }, [room.scope, workspace])

  const load = useCallback(() => {
    if (!workspace || !canApprove) {
      setRows([])
      return
    }
    fetchOpenProposals(daemonCliGet, room.scope, workspace).then(setRows, (e: unknown) => {
      // An older server has no proposals route: nothing to show.
      console.warn('[clone-proposals] list:', e)
      setRows([])
    })
  }, [room.scope, workspace, canApprove])

  useEffect(() => {
    load()
  }, [load, refreshKey])

  if (rows.length === 0) return null

  const review = (p: CloneProposal): void => {
    const host = hostForTarget(hosts, p.target)
    if (!host || !p.workspacePath) return
    const path = p.workspacePath
    // Loaded on demand: the clone flow (dialog store, projects store) stays
    // out of every Thread pane's module graph.
    void import('@/lib/start-clone-to').then(({ startCloneProposal }) =>
      startCloneProposal(p, path, p.workspaceHandle ?? workspace ?? 'workspace', host),
    )
  }
  const deny = async (p: CloneProposal): Promise<void> => {
    setBusy(p.id)
    setError(null)
    try {
      await denyProposal(daemonCliPost, room.scope, p.id)
      load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }
  // The dialog runs a clone from this window's own server only.
  const sameServer = room.scope.isPrimary

  return (
    <div className="border-b border-[var(--color-border)] px-3 py-2 space-y-2" data-testid="clone-proposals-banner">
      {rows.map((p) => {
        const host = hostForTarget(hosts, p.target)
        const runnable = sameServer && !!host && !!p.workspacePath
        return (
          <div key={p.id} className="flex items-start gap-3" data-testid={`clone-proposal-${p.id}`}>
            <div className="min-w-0 flex-1">
              <p className="text-xs text-[var(--color-text-primary)]">
                {proposerName(p)} proposes a clone to {p.target === 'here' ? 'a second copy on this server' : p.target}
              </p>
              <p className="text-[11px] text-[var(--color-text-muted)] break-words">
                {p.why ? `${p.why} · ` : ''}
                {p.summary}
              </p>
              {!runnable && (
                <p className="text-[11px] text-[var(--color-text-muted)]" data-testid="clone-proposal-cli-hint">
                  {p.target === 'here' || !host
                    ? `Run it from a terminal: k2 clone approve ${p.id}`
                    : `Open this agent on its own server to review it, or run k2 clone approve ${p.id}`}
                </p>
              )}
            </div>
            <div className="flex flex-shrink-0 items-center gap-2">
              {runnable && (
                <Button variant="accent" onClick={() => review(p)} disabled={busy === p.id}>
                  Review clone…
                </Button>
              )}
              <Button variant="ghost" onClick={() => void deny(p)} disabled={busy === p.id}>
                Deny
              </Button>
            </div>
          </div>
        )
      })}
      {error && (
        <p role="alert" className="text-[11px] text-[var(--color-status-error)]">
          {error}
        </p>
      )}
    </div>
  )
}
