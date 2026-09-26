// Settings → Token usage. Machine total plus the open workspace.
// Numbers come from GET /cli/usage/tokens. The renderer does not read
// transcript files. No prices and no subscription percents.

import React, { useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import type { SettingEntry } from '../searchManifest'

export const TOKEN_USAGE_MANIFEST: SettingEntry[] = [
  {
    id: 'token-usage.machine',
    section: 'token-usage',
    label: 'Token usage',
    description: 'Token counts for this machine, by workspace, harness, and model',
    keywords: ['tokens', 'usage', 'claude', 'codex', 'grok', 'cache', 'input', 'output'],
  },
]

interface TokenTotals {
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  turns: number
}

interface UsageReport {
  scope: string
  workspace: string | null
  total: TokenTotals
  harnesses: Array<TokenTotals & { harness: string }>
  models: Array<TokenTotals & { harness: string; model: string }>
  workspaces: Array<TokenTotals & { path: string; outside: boolean }>
  outside: TokenTotals
}

const EMPTY: TokenTotals = {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  turns: 0,
}

function fmt(n: number): string {
  return n.toLocaleString()
}

function harnessLabel(harness: string): string {
  if (harness === 'claude') return 'Claude'
  if (harness === 'codex') return 'Codex'
  if (harness === 'grok') return 'Grok'
  return harness || 'unknown'
}

function modelLabel(model: string): string {
  return model === '' ? 'unknown' : model
}

function TotalsTable({
  rows,
  labelHeader,
  label,
}: {
  rows: Array<{ key: string; label: string; totals: TokenTotals }>
  labelHeader: string
  label: (row: { key: string; label: string }) => string
}): React.JSX.Element {
  if (rows.length === 0) {
    return <p className="text-xs text-[var(--color-text-muted)]">No counts yet.</p>
  }
  return (
    <table className="w-full text-xs border-collapse">
      <thead>
        <tr className="text-left text-[var(--color-text-muted)]">
          <th className="py-1 pr-3 font-medium">{labelHeader}</th>
          <th className="py-1 pr-3 font-medium text-right">Input</th>
          <th className="py-1 pr-3 font-medium text-right">Output</th>
          <th className="py-1 pr-3 font-medium text-right">Cache read</th>
          <th className="py-1 pr-3 font-medium text-right">Cache write</th>
          <th className="py-1 font-medium text-right">Turns</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.key} className="border-t border-[var(--color-border)]">
            <td className="py-1.5 pr-3">{label(row)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.input_tokens)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.output_tokens)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.cache_read_tokens)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.cache_write_tokens)}</td>
            <td className="py-1.5 text-right tabular-nums">{fmt(row.totals.turns)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function ReportBlock({ title, path, report }: { title: string; path?: string; report: UsageReport }): React.JSX.Element {
  const harnessRows = report.harnesses.map((h) => ({
    key: h.harness,
    label: h.harness,
    totals: h,
  }))
  const modelRows = report.models.map((m) => ({
    key: `${m.harness}:${m.model}`,
    label: `${harnessLabel(m.harness)} · ${modelLabel(m.model)}`,
    totals: m,
  }))
  return (
    <section className="mb-8">
      <h3 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">{title}</h3>
      {path ? (
        <p className="text-[11px] font-mono text-[var(--color-text-muted)] mb-3 break-all">{path}</p>
      ) : null}
      <p className="text-xs text-[var(--color-text-muted)] mb-3">
        {fmt(report.total.turns)} turns · input {fmt(report.total.input_tokens)} · output{' '}
        {fmt(report.total.output_tokens)} · cache read {fmt(report.total.cache_read_tokens)} · cache write{' '}
        {fmt(report.total.cache_write_tokens)}
      </p>
      <h4 className="text-xs font-medium mb-1">Harness</h4>
      <TotalsTable rows={harnessRows} labelHeader="Harness" label={(row) => harnessLabel(row.label)} />
      <h4 className="text-xs font-medium mt-4 mb-1">Model</h4>
      <TotalsTable rows={modelRows} labelHeader="Model" label={(row) => row.label} />
    </section>
  )
}

export function TokenUsageSection(): React.JSX.Element {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const project = projects.find((p) => p.id === activeProjectId)
  const workspace = project?.workspaces.find((w) => w.id === activeWorkspaceId)
  const openCwd = workspace?.worktreePath ?? project?.path ?? ''

  const [machine, setMachine] = useState<UsageReport | null>(null)
  const [openReport, setOpenReport] = useState<UsageReport | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const ac = new AbortController()
    setError(null)
    setMachine(null)
    setOpenReport(null)
    void (async () => {
      try {
        const total = await daemonCliGet<UsageReport>('usage/tokens')
        if (ac.signal.aborted) return
        setMachine(total)
        if (openCwd) {
          const one = await daemonCliGet<UsageReport>('usage/tokens', { workspace: openCwd })
          if (ac.signal.aborted) return
          setOpenReport(one)
        }
      } catch (err) {
        if (ac.signal.aborted) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
      }
    })()
    return () => ac.abort()
  }, [hostKey, openCwd])

  const ownerOnly = error != null && /auth token|forbidden/i.test(error)

  return (
    <div className="max-w-3xl">
      <h2 className="text-base font-medium text-[var(--color-text-primary)] mb-1">Token usage</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-2">
        Counts stored on this machine from Claude, Codex, and Grok transcripts. One row per counted
        turn. No prices. No subscription percents.
      </p>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Claude input is the uncached part the transcript stored, and Claude output may still be an
        early streaming placeholder. Cache read is the trustworthy Claude figure.
      </p>
      {ownerOnly ? (
        <p className="text-sm text-[var(--color-text-muted)]">Token usage is visible to the server owner.</p>
      ) : error ? (
        <p className="text-sm text-[var(--color-text-muted)]">{error}</p>
      ) : !machine ? (
        <p className="text-sm text-[var(--color-text-muted)]">Loading…</p>
      ) : (
        <>
          <ReportBlock title="This machine" report={machine} />
          <h4 className="text-xs font-medium mb-1">Workspace</h4>
          <TotalsTable
            rows={[
              ...machine.workspaces
                .filter((w) => !w.outside)
                .map((w) => ({ key: w.path, label: w.path, totals: w })),
              {
                key: 'outside',
                label: 'Outside workspaces',
                totals: machine.outside ?? EMPTY,
              },
            ]}
            labelHeader="Path"
            label={(row) => row.label}
          />
          <div className="mt-8">
            {openCwd && openReport ? (
              <ReportBlock title="Open workspace" path={openReport.workspace ?? openCwd} report={openReport} />
            ) : (
              <section>
                <h3 className="text-sm font-medium mb-1">Open workspace</h3>
                <p className="text-xs text-[var(--color-text-muted)]">No workspace is open.</p>
              </section>
            )}
          </div>
        </>
      )}
    </div>
  )
}
