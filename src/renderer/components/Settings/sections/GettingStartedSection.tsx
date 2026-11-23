// Settings → Getting started (plan-getting-started-settings-v1): start the
// setup list again on a server already in use, and get to Kessel there.
// Every person sees the setup list's part; the Kessel part is for the owner,
// Owners and Admins. The daemon decides and answers everything
// (`first-run/status`, `first-run/restart`, `first-run/kessel/welcome`); this
// page draws its answer and sends the gestures.
//
// - Start setup again: restart, then Settings closes so the list shows. A
//   Member while the list is off asks the owner on a ticket instead, and is
//   told so here.
// - Set up Kessel: restart with Kessel turned on, then the Kessel sheet.
// - Open Kessel: its Chat tab (the sheet's helper). Start Kessel's welcome
//   again: behind a short confirm.
//
// Rows and buttons are the Settings ones (SendFeedbackRows); no new look.

import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { openWorkspaceThread } from '@/lib/open-workspace-thread'
import { productName } from '@/lib/brand'
import { primaryScope } from '@/kessel/server-scope'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { rowsLeft, useFirstRunStore, watchFirstRun, type FirstRunDoc } from '@/stores/first-run'
import { openKesselSheet } from '@/stores/kessel-sheet'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'

export const GETTING_STARTED_MANIFEST: SettingEntry[] = [
  {
    id: 'getting-started.setup-list',
    section: 'getting-started',
    label: 'Start setup again',
    description: 'Bring back the setup list, skipped steps included',
    keywords: ['setup', 'set up', 'get started', 'getting started', 'checklist', 'restart', 'onboarding'],
  },
  {
    id: 'getting-started.kessel',
    section: 'getting-started',
    label: 'Kessel',
    description: 'Set up Kessel, open it, or start its welcome again',
    keywords: ['kessel', 'welcome', 'hello', 'orchestrator', 'chief of staff'],
  },
]

/** The page's words (purpose only). */
export const GETTING_STARTED_COPY = {
  setupHeading: 'Setup list',
  finished: 'Finished',
  off: 'Off on this server',
  startAgain: 'Start setup again',
  asked: 'The owner was asked on a ticket. Your setup starts again here when they say yes.',
  askedAlready: 'You asked already. Your setup starts again here when the owner or an admin says yes.',
  kesselHeading: 'Kessel',
  setUpKessel: 'Set up Kessel',
  openKessel: 'Open Kessel',
  welcomeAgain: 'Start Kessel’s welcome again',
  welcomeConfirm: 'Kessel greets you again and asks its first questions. Its notes, chats and permissions stay.',
  welcomeSent: 'Kessel is starting its welcome again in its Thread.',
} as const

/** The setup list's state line: "Finished", "{n} steps left", or "Off on
 *  this server". */
export function setupStateLine(doc: FirstRunDoc | null | undefined): string {
  if (!doc || !doc.enabled) return GETTING_STARTED_COPY.off
  if (doc.finished) return GETTING_STARTED_COPY.finished
  const n = rowsLeft(doc.rows)
  if (n === 0) return GETTING_STARTED_COPY.finished
  return n === 1 ? '1 step left' : `${n} steps left`
}

/** The Kessel part's line: what Kessel is for, or who does it here. */
export function kesselLine(name: string | null): string {
  return name
    ? `${name} runs ${productName()} for you.`
    : `Kessel runs ${productName()} for you: setup, settings and hiring your agents.`
}

/** The owner token, an Owner or an Admin, from `auth/whoami`. */
export function managesFromWhoami(who: unknown): boolean {
  if (!who || typeof who !== 'object') return false
  const w = who as { owner?: unknown; role?: unknown }
  return w.owner === true || w.role === 'owner' || w.role === 'admin'
}

function errorText(e: unknown): string {
  return (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
}

function ActionButton({
  testId,
  onClick,
  disabled,
  children,
}: {
  testId: string
  onClick: () => void
  disabled?: boolean
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-testid={testId}
      onClick={onClick}
      disabled={disabled}
      className="px-2 py-0.5 type-control text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 disabled:cursor-default"
    >
      {children}
    </button>
  )
}

function Row({ id, label, hint, children }: { id: string; label: string; hint: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]" data-settings-id={id}>
      <div className="flex-1 min-w-0 mr-3">
        <span className="type-row text-[var(--color-text-secondary)]">{label}</span>
        <p className="type-hint text-[var(--color-text-muted)] mt-0.5" data-testid={`${id}-line`}>
          {hint}
        </p>
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">{children}</div>
    </div>
  )
}

export function GettingStartedSection(): React.JSX.Element {
  const scope = primaryScope()
  const doc = useFirstRunStore((s) => s.entries[scope.id]?.doc ?? null)
  const [manages, setManages] = useState(false)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => watchFirstRun(scope), [scope])
  useEffect(() => {
    let live = true
    daemonCliGet<unknown>(scope, 'auth/whoami')
      .then((who) => {
        if (live) setManages(managesFromWhoami(who))
      })
      .catch(() => {
        if (live) setManages(false)
      })
    return () => {
      live = false
    }
  }, [scope])

  const run = useCallback(async (work: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    setNote(null)
    try {
      await work()
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [])

  const startAgain = (): void =>
    void run(async () => {
      const out = await useFirstRunStore.getState().restart({}, scope)
      if (out.kind === 'asked') {
        setNote(out.already ? GETTING_STARTED_COPY.askedAlready : GETTING_STARTED_COPY.asked)
        return
      }
      // The list shows in the sidebar: close Settings so it's there.
      useSettingsStore.getState().closeSettings()
    })

  const setUpKessel = (): void =>
    void run(async () => {
      await useFirstRunStore.getState().restart({ kessel: true }, scope)
      openKesselSheet()
    })

  const seat = doc?.kesselSeat ?? null
  const kesselId = seat?.exists ? seat.workspaceId : null

  const openKessel = (): void => {
    if (kesselId) void openWorkspaceThread(kesselId)
  }

  const welcomeAgain = (): void =>
    void run(async () => {
      const ok = await useConfirmDialogStore.getState().confirm({
        title: GETTING_STARTED_COPY.welcomeAgain,
        message: GETTING_STARTED_COPY.welcomeConfirm,
        confirmLabel: GETTING_STARTED_COPY.welcomeAgain,
      })
      if (!ok) return
      await useFirstRunStore.getState().kesselWelcome(scope)
      setNote(GETTING_STARTED_COPY.welcomeSent)
    })

  return (
    <div className="flex flex-col gap-6" data-testid="getting-started-section">
      <div data-settings-group="getting-started-setup">
        <h3 className="type-section uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1">{GETTING_STARTED_COPY.setupHeading}</h3>
        <Row id="getting-started.setup-list" label={GETTING_STARTED_COPY.setupHeading} hint={setupStateLine(doc)}>
          <ActionButton testId="getting-started-restart" onClick={startAgain} disabled={busy}>
            {GETTING_STARTED_COPY.startAgain}
          </ActionButton>
        </Row>
      </div>
      {manages ? (
        <div data-settings-group="getting-started-kessel">
          <h3 className="type-section uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1">{GETTING_STARTED_COPY.kesselHeading}</h3>
          <Row id="getting-started.kessel" label={GETTING_STARTED_COPY.kesselHeading} hint={kesselLine(kesselId ? seat?.name ?? 'Kessel' : null)}>
            {kesselId ? (
              <>
                <ActionButton testId="getting-started-open-kessel" onClick={openKessel} disabled={busy}>
                  {GETTING_STARTED_COPY.openKessel}
                </ActionButton>
                <ActionButton testId="getting-started-kessel-welcome" onClick={welcomeAgain} disabled={busy}>
                  {GETTING_STARTED_COPY.welcomeAgain}
                </ActionButton>
              </>
            ) : (
              <ActionButton testId="getting-started-set-up-kessel" onClick={setUpKessel} disabled={busy}>
                {GETTING_STARTED_COPY.setUpKessel}
              </ActionButton>
            )}
          </Row>
        </div>
      ) : null}
      {note ? (
        <p className="type-hint text-[var(--color-text-secondary)] m-0" role="status" data-testid="getting-started-note">
          {note}
        </p>
      ) : null}
      {error ? (
        <p className="type-hint text-[var(--color-status-error-soft)] m-0" role="alert" data-testid="getting-started-error">
          {error}
        </p>
      ) : null}
    </div>
  )
}
