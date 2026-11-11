// Settings → Compute: the small pieces every Compute part shares, so a status
// word, a machine's one-line description and a copyable command read the same
// in Machines, Nodes and This computer.

import React, { useState } from 'react'
import { Button } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import { StatusDot, type HealthTone } from './ConnectionsParts'
import { formatBytes, type NodeDot } from './compute-api'

/** A node's state in words, with the tone of its dot. */
export const NODE_WORD: Record<NodeDot, { word: string; tone: HealthTone }> = {
  online: { word: 'Online', tone: 'ok' },
  offline: { word: 'Offline', tone: 'neutral' },
  pending: { word: 'Waiting for your OK', tone: 'warn' },
  paused: { word: 'Paused', tone: 'warn' },
  draining: { word: 'Finishing its jobs', tone: 'warn' },
}

/** Dot plus word; the word carries the state, so it never relies on colour. */
export function NodeStatus({ dot, word }: { dot: NodeDot; word?: string }): React.JSX.Element {
  const w = NODE_WORD[dot]
  return (
    <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)] whitespace-nowrap">
      <StatusDot tone={w.tone} />
      <span data-testid="node-dot">{word ?? w.word}</span>
    </span>
  )
}

/** "macOS · Apple silicon", "Linux · x86-64"; an unknown value as sent. */
export function osLine(os: string | null | undefined, arch: string | null | undefined): string {
  const o = (os ?? '').toLowerCase()
  const a = (arch ?? '').toLowerCase()
  const osWord = o === 'macos' || o === 'darwin' ? 'macOS' : o === 'linux' ? 'Linux' : o === 'windows' ? 'Windows' : os ?? ''
  const archWord =
    a === 'aarch64' || a === 'arm64'
      ? osWord === 'macOS'
        ? 'Apple silicon'
        : 'Arm'
      : a === 'x86_64' || a === 'amd64'
        ? 'x86-64'
        : arch ?? ''
  return [osWord, archWord].filter(Boolean).join(' · ')
}

/** "10 cores · 16 GB RAM · 100 GB free". */
export function hardwareLine(cores: string, memBytes: number | null | undefined, diskFree: number | null | undefined): string {
  return `${cores} · ${formatBytes(memBytes)} RAM · ${formatBytes(diskFree)} free`
}

/** A command or code to copy: monospaced, selectable, with a Copy button that
 *  says Copied for a moment (or why it couldn't). */
export function CopyBox({ text, label, testId }: { text: string; label?: string; testId?: string }): React.JSX.Element {
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle')
  const copy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(text)
      setState('copied')
    } catch {
      setState('failed')
    }
    setTimeout(() => setState('idle'), 1800)
  }
  return (
    <div className="space-y-1" data-testid={testId}>
      {label ? <div className="type-hint text-[var(--color-text-secondary)]">{label}</div> : null}
      <div className="flex items-start gap-2">
        <code className="flex-1 min-w-0 break-all type-hint font-mono text-[var(--color-text-primary)] bg-[var(--color-bg)] border border-[var(--color-border)] rounded-[var(--radius-field)] px-2 py-1.5 select-all">
          {text}
        </code>
        <Button onClick={() => void copy()} aria-label={label ? `Copy ${label}` : 'Copy'}>
          {state === 'copied' ? 'Copied' : state === 'failed' ? 'Couldn’t copy' : 'Copy'}
        </Button>
      </div>
    </div>
  )
}

/** A text link to another Settings page, with the shared focus ring. */
export function SettingsLink({
  to,
  children,
  testId,
}: {
  to: string
  children: React.ReactNode
  testId?: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-k2-focus=""
      data-testid={testId}
      onClick={() => useSettingsStore.getState().openSettings(to as never)}
      className="relative px-0.5 -mx-0.5 type-hint text-[var(--color-accent-text)] underline underline-offset-2 no-drag cursor-pointer"
    >
      {children}
    </button>
  )
}

/** A plain error line for an action that failed: what failed, then the
 *  server's own reason quietly, then that it is safe to try again. */
export function ActionError({ what, reason }: { what: string; reason: string | null }): React.JSX.Element | null {
  if (!reason) return null
  const r = reason.trim().replace(/[.\s]+$/, '')
  return (
    <div role="alert" data-testid="compute-action-error" className="mt-2">
      <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{what} Nothing changed; try again.</p>
      <p className="type-hint text-[var(--color-text-muted)] mt-0.5 leading-relaxed max-w-[75ch] break-words">The server said: {r}.</p>
    </div>
  )
}
