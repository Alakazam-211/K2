// The K5 sheet (prd-setup-checklist-v1 §10.1, `k5-first-run-contract.md`):
// make K5, K2's own agent on this server, in two steps.
//
//   1. Name and folder: from the daemon's `k5Defaults`, all editable. The
//      folder is `<home>/k2-agents/<handle>`, so the handle names it.
//   2. What K5 thinks with: the claude, codex and grok tools signed in on
//      this server, with the server default chosen.
//
// Create K5 posts `first-run/k5`, then opens K5's Chat tab in the Thread
// view (asked for explicitly, not this client's "New agents open in"). The
// daemon decides who may (owner token, Owner, Admin) and refuses anything
// else; its refusal is shown as it says it. An Admin creator is told K5 asks
// an Owner before changing the organization (the daemon leaves
// `organization.manage` out of an Admin's grant).
//
// Opened by `useK5SheetStore` (the offer after a sign-in, the checklist's
// `k5_sheet`), mounted once in App. The window's server only.
//
// Look: the Settings dialogs' frame (Admin Access, the LLM sign-in sheet):
// one width, its top edge in one place, the title and × pinned while the
// body scrolls, labelled fields with a hint or problem under them, the
// footer's quiet answer before its main one. Each step is a form, so Enter
// moves on (Next, then Create K5) and Esc, the × or a press outside cancel;
// all three are held while K5 is being made.

import { useEffect, useId, useMemo, useState } from 'react'
import { Button, Callout, DialogFrame, DialogScrim, Input, Radio, Spinner, Surface, Tag } from '@/components/ui'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { AgentsIcon, FolderIcon } from '@/components/icons/K2Icons'
import { DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED, FailedNote } from '@/components/Settings/sections/AccessDialogs'
import { SETTING_ROW_LABEL } from '@/components/Settings/controls/SettingControls'
import { daemonCliGet } from '@/lib/daemon-cli'
import { openWorkspaceThread } from '@/lib/open-workspace-thread'
import { primaryScope } from '@/kessel/server-scope'
import { resolveAgentCommand } from '@/lib/agent-resolve'
import { useFirstRunStore, watchFirstRun, type K5Defaults } from '@/stores/first-run'
import { useK5SheetStore } from '@/stores/k5-sheet'
import { useLlmAccountsStore, watchLlmAccounts, type LlmAccountsDoc } from '@/stores/llm-accounts'
import { requestLlmSignIn } from '@/stores/llm-sign-in-request'
import { usePresetsStore } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'

/** The contract's copy (purpose only). */
export const K5_SHEET_COPY = {
  step1: 'Name and folder',
  step2: 'What K5 thinks with',
  adminCreator: 'K5 will ask an Owner before changing your organization.',
  create: 'Create K5',
} as const

/** The line under each step's title, and the sheet's other words. */
export const K5_STEP_INTRO = {
  step1: 'K5 is K2’s own agent. It works in its own folder on this server.',
  step2: 'K5 uses one of the tools signed in on this server.',
} as const

/** The title while there's no step to show (loading, switched off). */
export const K5_SHEET_TITLE = 'Meet K5'

/** The tools K5 can think with, in the checklist's order. */
export const K5_TOOLS = ['claude', 'codex', 'grok'] as const

export const K5_NO_TOOL_TEXT = 'None is signed in yet. Sign in to Claude, Codex or Grok, then come back to make K5.'
export const K5_EXISTS_TEXT = 'K5 is already on this server.'
/** The already-there view: its title and line. */
export const K5_EXISTS_TITLE = 'K5 is already on this server'
export const K5_EXISTS_INTRO = 'Open K5 to talk with it in its Thread.'
export const K5_OFF_TEXT = "K5 isn't turned on for this server."
export const K5_CREATE_FAILED = {
  what: 'K2 couldn’t create K5.',
  next: 'Nothing was made. Try again in a moment.',
} as const

export interface K5Tool {
  tool: string
  display: string
}

/** The claude, codex and grok tools with a signed-in subscription or an
 *  API token (the `llm_sign_in` row's rule). */
export function signedInK5Tools(doc: LlmAccountsDoc | null | undefined): K5Tool[] {
  if (!doc) return []
  const out: K5Tool[] = []
  for (const name of K5_TOOLS) {
    const t = doc.tools.find((x) => x.tool === name)
    if (!t) continue
    if (t.accounts.some((a) => a.state === 'signed_in' || a.kind === 'api_key')) {
      out.push({ tool: t.tool, display: t.display || t.tool })
    }
  }
  return out
}

/** The server default when it's one of `tools`, else the first of them. */
export function defaultK5Tool(tools: readonly K5Tool[], serverDefault: string | null | undefined): string | null {
  if (serverDefault && tools.some((t) => t.tool === serverDefault)) return serverDefault
  return tools[0]?.tool ?? null
}

/** The folder K5 gets for `handle`: the default folder's parent plus the
 *  handle (the daemon's `<home>/k2-agents/<handle>`). */
export function k5FolderFor(defaults: K5Defaults | null, handle: string): string {
  const h = handle.trim()
  const path = defaults?.path ?? ''
  const cut = path.lastIndexOf('/')
  const parent = cut > 0 ? path.slice(0, cut) : '~/k2-agents'
  return h ? `${parent}/${h}` : `${parent}/`
}

/** Admin rather than Owner or the owner token, from `auth/whoami`. */
export function isAdminCreator(who: unknown): boolean {
  if (!who || typeof who !== 'object') return false
  const w = who as { owner?: unknown; role?: unknown }
  return w.owner !== true && w.role === 'admin'
}

/** A handle the daemon can name a folder with: no slashes, not `.`/`..`. */
export function handleProblem(handle: string): string | null {
  const h = handle.trim()
  if (!h) return 'Give K5 a folder name.'
  if (/[/\\]/.test(h) || h === '.' || h === '..') return "The folder name can't have slashes."
  return null
}

/** A name to show: anything but blank. */
export function nameProblem(name: string): string | null {
  return name.trim() ? null : 'Give K5 a name.'
}

/** A refusal as the sheet says it: K5 already there and the switch being
 *  off in the sheet's words, anything else in the server's. */
function refusal(e: unknown): { exists: boolean; text: string } {
  const code = (e as { code?: unknown } | null)?.code
  if (code === 'k5_exists') return { exists: true, text: K5_EXISTS_TEXT }
  if (code === 'k5_first_run_off') return { exists: false, text: K5_OFF_TEXT }
  return { exists: false, text: (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '') }
}

/** The server's default agent as a tool name (`claude`), from Settings'
 *  `defaultAgent` and the presets. */
function useServerDefaultTool(): string | null {
  const presets = usePresetsStore((s) => s.presets)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  return useMemo(() => resolveAgentCommand(presets, defaultAgent)?.command ?? defaultAgent ?? null, [presets, defaultAgent])
}

export default function K5Sheet(): React.JSX.Element | null {
  const isOpen = useK5SheetStore((s) => s.isOpen)
  if (!isOpen) return null
  return <K5SheetBody />
}

/** The frame every state shares: the step (when there is one), the title
 *  and its line pinned beside the ×, then the body, which scrolls. */
function K5Frame({
  title,
  step,
  intro,
  busy,
  onCancel,
  children,
}: {
  title: string
  step?: 1 | 2
  intro?: string
  busy: boolean
  onCancel: () => void
  children: React.ReactNode
}): React.JSX.Element {
  const titleId = useId()
  const introId = useId()
  return (
    <>
      <DialogScrim />
      <DialogFrame
        onCancel={onCancel}
        cancelHeld={busy}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={intro ? introId : undefined}
        aria-busy={busy || undefined}
        data-testid="k5-sheet"
        placement="top"
        className="w-[min(36rem,calc(100vw-2.5rem))] max-h-[calc(100vh-var(--dialog-top)-1rem)] flex flex-col"
      >
        <div className="flex-shrink-0 px-5 pt-5 pb-4 pr-10">
          {step ? (
            <p className="type-hint text-[var(--color-text-secondary)] mb-1" data-testid="k5-step">
              Step {step} of 2
            </p>
          ) : null}
          <h2 id={titleId} className="m-0 type-heading text-[var(--color-text-primary)]">
            {title}
          </h2>
          {intro ? (
            <p id={introId} className={`${DIALOG_MUTED} mt-1`} data-testid="k5-intro">
              {intro}
            </p>
          ) : null}
        </div>
        <div className="min-h-0 overflow-y-auto px-5 pb-5 space-y-4">{children}</div>
      </DialogFrame>
    </>
  )
}

/** A folder path: monospaced, selectable, wrapped anywhere (never cut). */
function FolderPath({ path, testId }: { path: string; testId?: string }): React.JSX.Element {
  return (
    <span className="flex items-start gap-1.5 type-hint leading-relaxed text-[var(--color-text-secondary)]">
      <FolderIcon size={12} className="mt-[3px] flex-shrink-0 text-[var(--color-text-muted)]" />
      <span className="min-w-0 font-mono select-text [overflow-wrap:anywhere]" data-testid={testId}>
        {path}
      </span>
    </span>
  )
}

function K5SheetBody(): React.JSX.Element {
  const close = useK5SheetStore((s) => s.close)
  const scope = primaryScope()
  useEffect(() => watchFirstRun(scope), [scope])
  useEffect(() => watchLlmAccounts(scope), [scope])
  const ids = { name: useId(), handle: useId(), handleHint: useId() }

  const firstRun = useFirstRunStore((s) => s.entries[scope.id])
  const doc = firstRun?.doc ?? null
  const defaults = doc?.k5Defaults ?? null
  const seat = doc?.k5 ?? null
  const accounts = useLlmAccountsStore((s) => s.entries[scope.id]?.doc)
  const tools = useMemo(() => signedInK5Tools(accounts), [accounts])
  const serverDefault = useServerDefaultTool()

  const [step, setStep] = useState<1 | 2>(1)
  const [name, setName] = useState('')
  const [handle, setHandle] = useState('')
  const [touched, setTouched] = useState(false)
  const [tool, setTool] = useState<string | null>(null)
  const [admin, setAdmin] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{ exists: boolean; text: string } | null>(null)

  // Fill step 1 from the daemon's defaults until the person types.
  useEffect(() => {
    if (!defaults || touched) return
    setName(defaults.name)
    setHandle(defaults.handle)
  }, [defaults, touched])

  // The server default, until the person picks (or it stops being signed in).
  useEffect(() => {
    if (tool && tools.some((t) => t.tool === tool)) return
    setTool(defaultK5Tool(tools, serverDefault))
  }, [tools, serverDefault, tool])

  // Who is making K5: the daemon's own answer, as other surfaces read it.
  useEffect(() => {
    let cancelled = false
    daemonCliGet<unknown>(scope, 'auth/whoami')
      .then((who) => {
        if (!cancelled) setAdmin(isAdminCreator(who))
      })
      .catch(() => {
        /* unknown: no line (the daemon still decides what K5 is granted) */
      })
    return () => {
      cancelled = true
    }
  }, [scope])

  const problem = handleProblem(handle)
  const noName = nameProblem(name)
  const loading = !doc && !firstRun?.error
  const accountsLoading = !accounts

  const next = (): void => {
    if (problem || noName) {
      setTouched(true)
      return
    }
    setStep(2)
  }

  const create = async (): Promise<void> => {
    if (!tool || problem || noName || busy) return
    setBusy(true)
    setError(null)
    try {
      const made = await useFirstRunStore.getState().createK5({ tool, name: name.trim(), handle: handle.trim() }, scope)
      close()
      void openWorkspaceThread(made.workspaceId)
    } catch (e) {
      const why = refusal(e)
      setError(why)
      // Someone made K5 meanwhile: read the status, so the sheet shows the
      // K5 that's there and opens it.
      if (why.exists) void useFirstRunStore.getState().load(scope)
    } finally {
      setBusy(false)
    }
  }

  const openK5 = (): void => {
    if (!seat) return
    close()
    void openWorkspaceThread(seat.workspaceId)
  }

  const signIn = (): void => {
    close()
    requestLlmSignIn()
  }

  if (seat) {
    return (
      <K5Frame title={K5_EXISTS_TITLE} intro={K5_EXISTS_INTRO} busy={false} onCancel={close}>
        <Surface chamfer material="tile" className="flex items-start gap-2.5 px-4 py-2.5" data-testid="k5-exists">
          <AgentsIcon size={16} className="mt-px flex-shrink-0 text-[var(--color-accent-text)]" />
          <div className="min-w-0 space-y-0.5">
            <p className={SETTING_ROW_LABEL}>{seat.name}</p>
            {seat.path ? <FolderPath path={seat.path} /> : null}
          </div>
        </Surface>
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={close}>
            Close
          </Button>
          <Button variant="accent" size="md" onClick={openK5} autoFocus data-testid="k5-open">
            Open {seat.name}
          </Button>
        </div>
      </K5Frame>
    )
  }

  if (loading || !defaults) {
    return (
      <K5Frame title={K5_SHEET_TITLE} busy={false} onCancel={close}>
        {loading ? (
          <div className="flex items-center gap-2" role="status" data-testid="k5-loading">
            <Spinner />
            <span className={DIALOG_MUTED}>Loading…</span>
          </div>
        ) : (
          <p className={DIALOG_MUTED} data-testid="k5-off">
            {firstRun?.error ?? K5_OFF_TEXT}
          </p>
        )}
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={close}>
            Close
          </Button>
        </div>
      </K5Frame>
    )
  }

  if (step === 1) {
    const showHandleProblem = touched && problem
    return (
      <K5Frame title={K5_SHEET_COPY.step1} step={1} intro={K5_STEP_INTRO.step1} busy={false} onCancel={close}>
        <form
          className="space-y-4"
          noValidate
          onSubmit={(e) => {
            e.preventDefault()
            next()
          }}
        >
          <div>
            <label htmlFor={ids.name} className={DIALOG_LABEL}>
              Name
            </label>
            <Input
              id={ids.name}
              autoFocus
              className={DIALOG_FIELD}
              value={name}
              autoComplete="off"
              aria-invalid={touched && noName ? true : undefined}
              onChange={(e) => {
                setTouched(true)
                setName(e.target.value)
              }}
              data-testid="k5-name"
            />
            {touched && noName ? (
              <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="k5-name-problem">
                {noName}
              </p>
            ) : null}
          </div>
          <div>
            <label htmlFor={ids.handle} className={DIALOG_LABEL}>
              Folder name
            </label>
            <Input
              id={ids.handle}
              className={`${DIALOG_FIELD} font-mono`}
              value={handle}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={showHandleProblem ? true : undefined}
              aria-describedby={ids.handleHint}
              onChange={(e) => {
                setTouched(true)
                setHandle(e.target.value)
              }}
              data-testid="k5-handle"
            />
            <div id={ids.handleHint} className="mt-1">
              {showHandleProblem ? (
                <p className="type-hint text-[var(--color-status-error-soft)]" data-testid="k5-handle-problem">
                  {problem}
                </p>
              ) : (
                <FolderPath path={k5FolderFor(defaults, handle)} testId="k5-folder" />
              )}
            </div>
          </div>
          <div className="flex justify-end gap-2 pt-1">
            <Button variant="ghost" size="md" onClick={close}>
              Cancel
            </Button>
            <Button type="submit" variant="accent" size="md" disabled={!!problem || !!noName} data-testid="k5-next">
              Next
            </Button>
          </div>
        </form>
      </K5Frame>
    )
  }

  const noTool = !accountsLoading && tools.length === 0
  return (
    <K5Frame title={K5_SHEET_COPY.step2} step={2} intro={K5_STEP_INTRO.step2} busy={busy} onCancel={close}>
      <form
        className="space-y-4"
        noValidate
        onSubmit={(e) => {
          e.preventDefault()
          if (noTool) signIn()
          else void create()
        }}
        onKeyDown={(e) => {
          // Enter on a tool makes K5, as it would in a text field.
          if (e.key === 'Enter' && (e.target as HTMLInputElement).type === 'radio') {
            e.preventDefault()
            void create()
          }
        }}
      >
        {accountsLoading ? (
          <div className="flex items-center gap-2" role="status" data-testid="k5-tools-loading">
            <Spinner />
            <span className={DIALOG_MUTED}>Loading…</span>
          </div>
        ) : noTool ? (
          <p className={DIALOG_MUTED} data-testid="k5-no-tool">
            {K5_NO_TOOL_TEXT}
          </p>
        ) : (
          <div role="radiogroup" aria-label={K5_SHEET_COPY.step2} className="space-y-2" data-testid="k5-tools">
            {tools.map((t) => (
              <label key={t.tool} className="flex items-start gap-2 cursor-pointer" data-k5-tool={t.tool}>
                <Radio
                  name="k5-tool"
                  className="mt-0.5"
                  value={t.tool}
                  checked={tool === t.tool}
                  autoFocus={tool === t.tool}
                  disabled={busy}
                  onChange={() => setTool(t.tool)}
                  data-testid={`k5-tool-${t.tool}`}
                />
                <span className={`${SETTING_ROW_LABEL} inline-flex items-center gap-1.5`}>
                  <AgentIcon agent={t.tool} size={14} />
                  {t.display}
                  {t.tool === serverDefault ? <Tag className="ml-1">Server default</Tag> : null}
                </span>
              </label>
            ))}
          </div>
        )}
        {admin ? (
          <Callout tone="info" className="type-hint" data-testid="k5-admin-line">
            {K5_SHEET_COPY.adminCreator}
          </Callout>
        ) : null}
        {error ? (
          <div data-testid="k5-error">
            {error.exists ? (
              <Callout tone="info" role="status" className="type-hint">
                {error.text}
              </Callout>
            ) : (
              <FailedNote what={K5_CREATE_FAILED.what} next={K5_CREATE_FAILED.next} detail={error.text} />
            )}
          </div>
        ) : null}
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" disabled={busy} onClick={() => setStep(1)} data-testid="k5-back">
            Back
          </Button>
          {noTool ? (
            <Button type="submit" variant="accent" size="md" autoFocus data-testid="k5-sign-in">
              Sign in
            </Button>
          ) : (
            <Button type="submit" variant="accent" size="md" disabled={busy || !tool || accountsLoading} data-testid="k5-create">
              {busy ? 'Creating K5…' : K5_SHEET_COPY.create}
            </Button>
          )}
        </div>
      </form>
    </K5Frame>
  )
}
