// The Kessel sheet (prd-setup-checklist-v1 §10.1, `kessel-first-run-contract.md`):
// make Kessel, K2's own agent on this server, in two steps.
//
//   1. Name and folder: from the daemon's `kesselDefaults`, all editable. The
//      folder is `<home>/k2-agents/<handle>`, so the handle names it. With
//      focus groups on, the focus group Kessel joins: Workspace settings' own
//      Focus group picker, the active group chosen.
//   2. What Kessel thinks with: the claude, codex and grok tools signed in on
//      this server, with the server default chosen.
//
// Create Kessel posts `first-run/kessel` (with `focusGroup` when one is chosen), then opens Kessel's Chat tab in the Thread
// view (asked for explicitly, not this client's "New agents open in"). The
// daemon decides who may (owner token, Owner, Admin) and refuses anything
// else; its refusal is shown as it says it. An Admin creator is told Kessel asks
// an Owner before changing the organization (the daemon leaves
// `organization.manage` out of an Admin's grant).
//
// Opened by `useKesselSheetStore` (the offer after a sign-in, the checklist's
// `kessel_sheet`), mounted once in App. The window's server only.
//
// Look: the Settings dialogs' frame (Admin Access, the LLM sign-in sheet):
// one width, centred in the window, the title and × pinned while the
// body scrolls, labelled fields with a hint or problem under them, the
// footer's quiet answer before its main one. Each step is a form, so Enter
// moves on (Next, then Create Kessel) and Esc, the × or a press outside cancel;
// all three are held while Kessel is being made.

import { useEffect, useId, useMemo, useState } from 'react'
import { Button, Callout, DialogFrame, DialogScrim, Input, Radio, Spinner, Surface, Tag } from '@/components/ui'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { AgentsIcon, FolderIcon } from '@/components/icons/K2Icons'
import { DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED, FailedNote } from '@/components/Settings/sections/AccessDialogs'
import { SETTING_ROW_LABEL, SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { daemonCliGet } from '@/lib/daemon-cli'
import { openWorkspaceThread } from '@/lib/open-workspace-thread'
import { primaryScope } from '@/kessel/server-scope'
import { resolveAgentCommand } from '@/lib/agent-resolve'
import { useFirstRunStore, watchFirstRun, type KesselDefaults } from '@/stores/first-run'
import { useFocusGroupsStore, type FocusGroup } from '@/stores/focus-groups'
import { useKesselSheetStore } from '@/stores/kessel-sheet'
import { useLlmAccountsStore, watchLlmAccounts, type LlmAccountsDoc } from '@/stores/llm-accounts'
import { requestLlmSignIn } from '@/stores/llm-sign-in-request'
import { usePresetsStore } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'

/** The contract's copy (purpose only). */
export const KESSEL_SHEET_COPY = {
  step1: 'Name and folder',
  step2: 'Choose Kessel’s tool',
  adminCreator: 'Kessel will ask an Owner before changing your Org Chart, which sets who leads which agents.',
  create: 'Create Kessel',
} as const

/** The line under each step's title, and the sheet's other words. */
export const KESSEL_STEP_INTRO = {
  step1: 'Kessel is K2’s own agent. It works in its own folder on this server.',
  step2: 'Kessel uses one of the tools signed in on this server.',
} as const

/** The title while there's no step to show (loading, switched off). */
export const KESSEL_SHEET_TITLE = 'Meet Kessel'

/** The tools Kessel can use, in the checklist's order. */
export const KESSEL_TOOLS = ['claude', 'codex', 'grok'] as const

export const KESSEL_NO_TOOL_TEXT = 'None is signed in yet. Sign in to Claude, Codex or Grok, then come back to make Kessel.'
export const KESSEL_EXISTS_TEXT = 'Kessel is already on this server.'
/** The already-there view: its title and line. */
export const KESSEL_EXISTS_TITLE = 'Kessel is already on this server'
export const KESSEL_EXISTS_INTRO = 'Open Kessel to talk with it in its Thread.'
export const KESSEL_OFF_TEXT = 'Kessel isn’t turned on for this server.'
/** Kessel was made by someone else while this person was on step 2. */
export const KESSEL_MADE_ELSEWHERE = {
  title: 'Someone else just made Kessel',
  intro: 'This one wasn’t made, since a server has one Kessel. Open it to talk with it in its Thread.',
  /** Until the status shows that Kessel (an older or slow server). */
  line: 'Someone else just made Kessel, so this one wasn’t made.',
} as const
/** Said beside the buttons while Kessel is being made. */
export const KESSEL_BUSY_TEXT = 'Making Kessel’s folder and starting it…'
/** The held ×’s tooltip while Kessel is being made. */
export const KESSEL_CLOSE_HELD = 'Can’t close while Kessel is being made'
/** The main button after a failed create. */
export const KESSEL_TRY_AGAIN = 'Try again'
export const KESSEL_CREATE_FAILED = {
  what: 'K2 couldn’t create Kessel.',
  next: 'Nothing was made. Fix what the server says, then try again.',
  /** The server couldn't write Kessel's folder. */
  permission: 'Nothing was made. K2 can’t write to that folder. Choose another folder name, or ask whoever runs this server to check its permissions.',
} as const

/** What to do after a failed create: a folder K2 can't write gets its own
 *  next step; anything else the general one. */
export function createFailedNext(reason: string): string {
  return /permission denied|EACCES|EPERM|not writable|read-only file system/i.test(reason) ? KESSEL_CREATE_FAILED.permission : KESSEL_CREATE_FAILED.next
}

export interface KesselTool {
  tool: string
  display: string
}

/** The claude, codex and grok tools with a signed-in subscription or an
 *  API token (the `llm_sign_in` row's rule). */
export function signedInKesselTools(doc: LlmAccountsDoc | null | undefined): KesselTool[] {
  if (!doc) return []
  const out: KesselTool[] = []
  for (const name of KESSEL_TOOLS) {
    const t = doc.tools.find((x) => x.tool === name)
    if (!t) continue
    if (t.accounts.some((a) => a.state === 'signed_in' || a.kind === 'api_key')) {
      out.push({ tool: t.tool, display: t.display || t.tool })
    }
  }
  return out
}

/** The server default when it's one of `tools`, else the first of them. */
export function defaultKesselTool(tools: readonly KesselTool[], serverDefault: string | null | undefined): string | null {
  if (serverDefault && tools.some((t) => t.tool === serverDefault)) return serverDefault
  return tools[0]?.tool ?? null
}

/** The folder Kessel gets for `handle`: the default folder's parent plus the
 *  handle (the daemon's `<home>/k2-agents/<handle>`). */
export function kesselFolderFor(defaults: KesselDefaults | null, handle: string): string {
  const h = handle.trim()
  const path = defaults?.path ?? ''
  const cut = path.lastIndexOf('/')
  const parent = cut > 0 ? path.slice(0, cut) : '~/k2-agents'
  return h ? `${parent}/${h}` : `${parent}/`
}

/** Workspace settings' Focus group options: No group, then each group. */
export function focusGroupOptions(groups: readonly FocusGroup[]): { value: string; label: string }[] {
  return [{ value: '', label: 'No group' }, ...groups.map((g) => ({ value: g.id, label: g.name }))]
}

/** The group the picker starts on: the active one while it's a group,
 *  else No group. */
export function defaultKesselFocusGroup(groups: readonly FocusGroup[], active: string | null): string {
  return active && groups.some((g) => g.id === active) ? active : ''
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
  if (!h) return 'Give Kessel a folder name.'
  if (/[/\\]/.test(h) || h === '.' || h === '..') return 'The folder name can’t have slashes.'
  return null
}

/** A name to show: anything but blank. */
export function nameProblem(name: string): string | null {
  return name.trim() ? null : 'Give Kessel a name.'
}

/** A refusal as the sheet says it: Kessel already there and the switch being
 *  off in the sheet's words, anything else in the server's. */
function refusal(e: unknown): { exists: boolean; text: string } {
  const code = (e as { code?: unknown } | null)?.code
  if (code === 'kessel_exists') return { exists: true, text: KESSEL_MADE_ELSEWHERE.line }
  if (code === 'kessel_first_run_off') return { exists: false, text: KESSEL_OFF_TEXT }
  return { exists: false, text: (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '') }
}

/** The server's default agent as a tool name (`claude`), from Settings'
 *  `defaultAgent` and the presets. */
function useServerDefaultTool(): string | null {
  const presets = usePresetsStore((s) => s.presets)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  return useMemo(() => resolveAgentCommand(presets, defaultAgent)?.command ?? defaultAgent ?? null, [presets, defaultAgent])
}

/** Step 1 shows the Focus group picker: focus groups on, and one or more. */
function useShowsFocusGroups(): boolean {
  return useFocusGroupsStore((s) => s.focusGroupsEnabled && s.focusGroups.length > 0)
}

export default function KesselSheet(): React.JSX.Element | null {
  const isOpen = useKesselSheetStore((s) => s.isOpen)
  if (!isOpen) return null
  return <KesselSheetBody />
}

/** The frame every state shares: the step (when there is one), the title
 *  and its line pinned beside the ×, the body, which scrolls, and the sheet
 *  one height across steps and states (a min-height that holds the tallest
 *  usual state, a refusal, so the buttons stay put; taller with the Focus
 *  group picker, which step 1 then holds with a long path and both
 *  problems shown), and
 *  the footer: an optional status line, then the buttons. With `onSubmit`
 *  the body and footer are one form, so Enter answers it. */
function KesselFrame({
  title,
  step,
  intro,
  busy,
  onCancel,
  onSubmit,
  onKeyDown,
  status,
  footer,
  children,
}: {
  title: string
  step?: 1 | 2
  intro?: string
  busy: boolean
  onCancel: () => void
  onSubmit?: () => void
  onKeyDown?: (e: React.KeyboardEvent<HTMLFormElement>) => void
  status?: React.ReactNode
  footer: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  const titleId = useId()
  const introId = useId()
  const tall = useShowsFocusGroups()
  const inner = (
    <>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-1 space-y-4" data-testid="kessel-body">
        {children}
      </div>
      <div className="flex-shrink-0 flex items-center gap-3 px-5 pt-5 pb-5" data-testid="kessel-footer">
        <div className="min-w-0 flex-1">{status}</div>
        <div className="flex flex-shrink-0 gap-2">{footer}</div>
      </div>
    </>
  )
  return (
    <>
      <DialogScrim />
      <DialogFrame
        onCancel={onCancel}
        cancelHeld={busy}
        cancelHeldReason={busy ? KESSEL_CLOSE_HELD : undefined}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={intro ? introId : undefined}
        aria-busy={busy || undefined}
        data-testid="kessel-sheet"
        className={`w-[min(36rem,calc(100vw-2.5rem))] ${tall ? 'min-h-[min(24.25rem,calc(100vh-2.5rem))]' : 'min-h-[min(22.25rem,calc(100vh-2.5rem))]'} max-h-[calc(100vh-2.5rem)] flex flex-col`}
      >
        <div className="flex-shrink-0 px-5 pt-5 pb-4 pr-10">
          {step ? (
            <p className="type-hint text-[var(--color-text-secondary)] mb-1" data-testid="kessel-step">
              Step {step} of 2
            </p>
          ) : null}
          <h2 id={titleId} className="m-0 type-heading text-[var(--color-text-primary)]">
            {title}
          </h2>
          {intro ? (
            <p id={introId} className={`${DIALOG_MUTED} mt-1`} data-testid="kessel-intro">
              {intro}
            </p>
          ) : null}
        </div>
        {onSubmit ? (
          <form
            className="flex min-h-0 flex-1 flex-col"
            noValidate
            onKeyDown={onKeyDown}
            onSubmit={(e) => {
              e.preventDefault()
              onSubmit()
            }}
          >
            {inner}
          </form>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">{inner}</div>
        )}
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

/** Kessel and its folder, on a tile. */
function KesselTile({ name, path }: { name: string; path: string }): React.JSX.Element {
  return (
    <Surface chamfer material="tile" className="flex items-start gap-2.5 px-4 py-2.5" data-testid="kessel-exists">
      <AgentsIcon size={16} className="mt-px flex-shrink-0 text-[var(--color-accent-text)]" />
      <div className="min-w-0 space-y-0.5">
        <p className={SETTING_ROW_LABEL}>{name}</p>
        {path ? <FolderPath path={path} /> : null}
      </div>
    </Surface>
  )
}

function KesselSheetBody(): React.JSX.Element {
  const close = useKesselSheetStore((s) => s.close)
  const scope = primaryScope()
  useEffect(() => watchFirstRun(scope), [scope])
  useEffect(() => watchLlmAccounts(scope), [scope])
  const ids = { name: useId(), handle: useId(), handleHint: useId() }

  const firstRun = useFirstRunStore((s) => s.entries[scope.id])
  const doc = firstRun?.doc ?? null
  const defaults = doc?.kesselDefaults ?? null
  const seat = doc?.kessel ?? null
  const accounts = useLlmAccountsStore((s) => s.entries[scope.id]?.doc)
  const tools = useMemo(() => signedInKesselTools(accounts), [accounts])
  const serverDefault = useServerDefaultTool()
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)
  const showGroups = useShowsFocusGroups()

  const [step, setStep] = useState<1 | 2>(1)
  const [name, setName] = useState('')
  const [handle, setHandle] = useState('')
  const [touched, setTouched] = useState(false)
  const [tool, setTool] = useState<string | null>(null)
  /** The person's pick; null follows the active group. */
  const [pickedGroup, setPickedGroup] = useState<string | null>(null)
  const group = pickedGroup !== null && (pickedGroup === '' || focusGroups.some((g) => g.id === pickedGroup))
    ? pickedGroup
    : defaultKesselFocusGroup(focusGroups, activeFocusGroupId)
  const [admin, setAdmin] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{ exists: boolean; text: string } | null>(null)
  /** Create Kessel found Kessel already made (by someone else, meanwhile). */
  const [madeElsewhere, setMadeElsewhere] = useState(false)

  // Fill step 1 from the daemon's defaults until the person types.
  useEffect(() => {
    if (!defaults || touched) return
    setName(defaults.name)
    setHandle(defaults.handle)
  }, [defaults, touched])

  // The server default, until the person picks (or it stops being signed in).
  useEffect(() => {
    if (tool && tools.some((t) => t.tool === tool)) return
    setTool(defaultKesselTool(tools, serverDefault))
  }, [tools, serverDefault, tool])

  // Who is making Kessel: the daemon's own answer, as other surfaces read it.
  useEffect(() => {
    let cancelled = false
    daemonCliGet<unknown>(scope, 'auth/whoami')
      .then((who) => {
        if (!cancelled) setAdmin(isAdminCreator(who))
      })
      .catch(() => {
        /* unknown: no line (the daemon still decides what Kessel is granted) */
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
      const req = { tool, name: name.trim(), handle: handle.trim(), ...(showGroups && group ? { focusGroup: group } : {}) }
      const made = await useFirstRunStore.getState().createKessel(req, scope)
      close()
      void openWorkspaceThread(made.workspaceId)
    } catch (e) {
      const why = refusal(e)
      setError(why)
      // Someone made Kessel meanwhile: read the status, so the sheet shows the
      // Kessel that's there and opens it.
      if (why.exists) {
        setMadeElsewhere(true)
        void useFirstRunStore.getState().load(scope)
      }
    } finally {
      setBusy(false)
    }
  }

  const openKessel = (): void => {
    if (!seat) return
    close()
    void openWorkspaceThread(seat.workspaceId)
  }

  const signIn = (): void => {
    close()
    requestLlmSignIn()
  }

  if (seat) {
    const words = madeElsewhere ? KESSEL_MADE_ELSEWHERE : { title: KESSEL_EXISTS_TITLE, intro: KESSEL_EXISTS_INTRO }
    return (
      <KesselFrame
        title={words.title}
        intro={words.intro}
        busy={false}
        onCancel={close}
        footer={
          <>
            <Button variant="ghost" size="md" onClick={close}>
              Close
            </Button>
            <Button variant="accent" size="md" onClick={openKessel} autoFocus data-testid="kessel-open">
              Open {seat.name}
            </Button>
          </>
        }
      >
        <KesselTile name={seat.name} path={seat.path} />
      </KesselFrame>
    )
  }

  if (loading || !defaults) {
    return (
      <KesselFrame
        title={KESSEL_SHEET_TITLE}
        busy={false}
        onCancel={close}
        footer={
          <Button variant="ghost" size="md" onClick={close}>
            Close
          </Button>
        }
      >
        {loading ? (
          <div className="flex items-center gap-2" role="status" data-testid="kessel-loading">
            <Spinner />
            <span className={DIALOG_MUTED}>Loading…</span>
          </div>
        ) : (
          <p className={DIALOG_MUTED} data-testid="kessel-off">
            {firstRun?.error ?? KESSEL_OFF_TEXT}
          </p>
        )}
      </KesselFrame>
    )
  }

  if (step === 1) {
    const showHandleProblem = touched && problem
    return (
      <KesselFrame
        title={KESSEL_SHEET_COPY.step1}
        step={1}
        intro={KESSEL_STEP_INTRO.step1}
        busy={false}
        onCancel={close}
        onSubmit={next}
        footer={
          <>
            <Button variant="ghost" size="md" onClick={close} data-testid="kessel-cancel">
              Cancel
            </Button>
            <Button type="submit" variant="accent" size="md" disabled={!!problem || !!noName} data-testid="kessel-next">
              Next
            </Button>
          </>
        }
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
            data-testid="kessel-name"
          />
          {touched && noName ? (
            <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="kessel-name-problem">
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
            data-testid="kessel-handle"
          />
          <div id={ids.handleHint} className="mt-1">
            {showHandleProblem ? (
              <p className="type-hint text-[var(--color-status-error-soft)]" data-testid="kessel-handle-problem">
                {problem}
              </p>
            ) : (
              <FolderPath path={kesselFolderFor(defaults, handle)} testId="kessel-folder" />
            )}
          </div>
        </div>
        {showGroups ? (
          <div>
            <p className={DIALOG_LABEL}>Focus group</p>
            <SettingDropdown
              fullWidth
              ariaLabel="Focus group"
              value={group}
              options={focusGroupOptions(focusGroups)}
              onChange={setPickedGroup}
              triggerData={{ 'data-testid': 'kessel-focus-group' }}
            />
          </div>
        ) : null}
      </KesselFrame>
    )
  }

  const noTool = !accountsLoading && tools.length === 0
  const failed = !!error && !error.exists
  return (
    <KesselFrame
      title={KESSEL_SHEET_COPY.step2}
      step={2}
      intro={KESSEL_STEP_INTRO.step2}
      busy={busy}
      onCancel={close}
      onSubmit={() => (noTool ? signIn() : void create())}
      onKeyDown={(e) => {
        // Enter on a tool makes Kessel, as it would in a text field.
        if (e.key === 'Enter' && (e.target as HTMLInputElement).type === 'radio') {
          e.preventDefault()
          void create()
        }
      }}
      status={
        busy ? (
          <p className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]" role="status" data-testid="kessel-busy">
            <Spinner />
            <span className="min-w-0">{KESSEL_BUSY_TEXT}</span>
          </p>
        ) : null
      }
      footer={
        <>
          <Button variant="ghost" size="md" disabled={busy} onClick={() => setStep(1)} data-testid="kessel-back">
            Back
          </Button>
          {noTool ? (
            <Button type="submit" variant="accent" size="md" autoFocus data-testid="kessel-sign-in">
              Sign in
            </Button>
          ) : (
            <Button type="submit" variant="accent" size="md" disabled={busy || !tool || accountsLoading} data-testid="kessel-create">
              {busy ? 'Creating Kessel…' : failed ? KESSEL_TRY_AGAIN : KESSEL_SHEET_COPY.create}
            </Button>
          )}
        </>
      }
    >
      {accountsLoading ? (
        <div className="flex items-center gap-2" role="status" data-testid="kessel-tools-loading">
          <Spinner />
          <span className={DIALOG_MUTED}>Loading…</span>
        </div>
      ) : noTool ? (
        <p className={DIALOG_MUTED} data-testid="kessel-no-tool">
          {KESSEL_NO_TOOL_TEXT}
        </p>
      ) : (
        <div role="radiogroup" aria-label={KESSEL_SHEET_COPY.step2} className="space-y-2" data-testid="kessel-tools">
          {tools.map((t) => (
            <label key={t.tool} className="flex items-start gap-2 cursor-pointer" data-kessel-tool={t.tool}>
              <Radio
                name="kessel-tool"
                className="mt-0.5"
                value={t.tool}
                checked={tool === t.tool}
                autoFocus={tool === t.tool}
                disabled={busy}
                onChange={() => setTool(t.tool)}
                data-testid={`kessel-tool-${t.tool}`}
              />
              <span className={`${SETTING_ROW_LABEL} flex items-center gap-1.5`}>
                <span aria-hidden="true" className="flex w-5 flex-shrink-0 items-center justify-center" data-kessel-tool-icon="">
                  <AgentIcon agent={t.tool} size={14} />
                </span>
                {t.display}
                {t.tool === serverDefault ? <Tag className="ml-1">Server default</Tag> : null}
              </span>
            </label>
          ))}
        </div>
      )}
      {admin ? (
        <Callout tone="info" className="type-hint" data-testid="kessel-admin-line">
          {KESSEL_SHEET_COPY.adminCreator}
        </Callout>
      ) : null}
      {error ? (
        <div data-testid="kessel-error">
          {error.exists ? (
            <Callout tone="info" role="status" className="type-hint">
              {KESSEL_MADE_ELSEWHERE.line}
            </Callout>
          ) : (
            <FailedNote what={KESSEL_CREATE_FAILED.what} next={createFailedNext(error.text)} detail={error.text} />
          )}
        </div>
      ) : null}
    </KesselFrame>
  )
}
