// Settings → Cores → Multiplayer (Beta), prd-multiplayer-cores-settings-v1 §1.
//
// One switch per core, rendered from the daemon's `GET /cli/docs/cores`
// (never a hard-coded list), so this page and `k2 doc cores` always say the
// same thing. A click posts `settings/update {multiplayerCores: {<id>: on}}`
// to the focused server; the page refetches after its own write and on every
// `doc_cores_changed`. Turning a core off while files are open live asks
// first. A Member sees the switches disabled.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onDocCoresChanged } from '@/stores/session-events'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { isHostSwitchedError } from '@/lib/daemon-cli'
import {
  failedWriteBackLine,
  plainCore,
  fetchDocCores,
  multiplayerCoresAvailable,
  setBoardFilePreview,
  openNowLine,
  setDocCore,
  turnOffMessage,
  turnOffTitle,
  type DocCore,
  type DocCores,
} from './multiplayer-cores-api'
import { BetaTag, Button, Callout, Spinner, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingsLoadState, SettingSwitchRow } from '../controls/SettingControls'
import { ActionNote, readFailure, useActionNote } from './ActionNote'
import { LivePreviewSwitches } from '@/components/LiveView/LivePreviewSwitches'
import { EditorPreviewSwitch } from '@/components/Editor/EditorPreviewSwitch'

export const MULTIPLAYER_INTRO =
  "People and agents work in the same file at the same time, with presence, history and everyone's own undo. " +
  'Choose which kinds of files open this way on this server. It applies to everyone here; the rest open the usual way.'

export const MULTIPLAYER_MEMBER_LINE = "Only the server's owner or an Admin can change these."

export const MULTIPLAYER_BETA_LINE = 'Keep your work in git while multiplayer is new.'
/** A core that opens as a Garden (the Board): its own Beta line. */
export const MULTIPLAYER_GARDEN_BETA_LINE = "Now and then, export a board from its ⋯ menu to keep a copy while it's new."
/** The board-file switch's words; it carries a Beta tag on screen. */
export const BOARD_FILE_PREVIEW_NAME = 'Open board files in a Board tab'
/** The switch's accessible name: the words plus "(Beta)", which the tag says on screen. */
export const BOARD_FILE_PREVIEW_LABEL = `${BOARD_FILE_PREVIEW_NAME} (Beta)`
export const BOARD_FILE_PREVIEW_LINE =
  'A board file in a workspace opens in its own tab, where agents can work on it too. When this is off, the file shows a card instead. Single-person for now.'


export const MULTIPLAYER_MANIFEST: SettingEntry[] = [
  {
    id: 'multiplayer.cores',
    section: 'multiplayer',
    label: 'Multiplayer',
    description: 'Turn each multiplayer core on or off for this server: Markdown, Text and code, Any other file (PSD, CAD, 3D and .blend files), Board. And open .k5board files in a Board tab.',
    keywords: ['live docs', 'live editing', 'multiplayer', 'Markdown', 'cores', 'co-editing', 'Board', 'k5board', 'whiteboard', 'PSD', 'CAD', '3D', 'blend', 'DWG', 'STEP', 'STL', 'any other file', 'file lock', 'versions'],
  },
]

/** The page on a server without multiplayer cores: only this computer's switches, and no error. */
export const MULTIPLAYER_EDITORS_ONLY_INTRO = 'Preview switches for this computer.'

const CONTENT_WIDTH = 'max-w-[52rem]'

/** What was wrong with an answer K2 couldn't read, in words: the known
 *  case says so; a field-level one says only that a part was unreadable. */
function shapeLine(shape: string): string {
  if (/^the server's answer/.test(shape)) return `${shape.charAt(0).toUpperCase()}${shape.slice(1)}.`
  return 'Part of the server’s answer wasn’t in a form K2 can read.'
}

/** A switch's flip: `done` (the write landed), `cancelled` (the person said
 *  no in the dialog), or the error the write threw. */
type FlipResult = { ok: true } | { ok: false; cancelled: true } | { ok: false; cancelled: false; err: unknown }

/** Settings → Multiplayer, always reachable: the cores (and the owner's live
 *  preview switches) on a server with `docs-cores-v1`; on any server, the new
 *  editors' per-computer switches. */
export function MultiplayerSection({ scope = primaryScope() }: { scope?: ServerScope }): React.JSX.Element {
  if (multiplayerCoresAvailable(scope)) return <MultiplayerCoresPage scope={scope} />
  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-testid="multiplayer-section" data-cores="none">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Multiplayer</h2>
        <p className={SETTING_ROW_HINT}>{MULTIPLAYER_EDITORS_ONLY_INTRO}</p>
      </header>
      <EditorPreviewSwitch />
    </div>
  )
}

function MultiplayerCoresPage({ scope }: { scope: ServerScope }): React.JSX.Element {
  const [data, setData] = useState<DocCores | null>(null)
  const [loadError, setLoadError] = useState<unknown>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [retrying, setRetrying] = useState(false)

  const load = useCallback(async () => {
    try {
      setData(await fetchDocCores(scope))
      setLoadError(null)
    } catch (err) {
      // The window switched servers mid-read: the next server's read lands.
      if (isHostSwitchedError(err)) return
      setLoadError(err ?? new Error('unknown'))
    }
  }, [scope])

  const retry = useCallback(() => {
    setRetrying(true)
    void load().finally(() => setRetrying(false))
  }, [load])

  useEffect(() => {
    void load()
  }, [load])

  // Another person (or `k2 doc cores`) flipped a core on this server.
  useEffect(() => onDocCoresChanged(scope, () => void load()), [scope, load])
  // The board preview is a setting: another window's change comes as the
  // settings refetch every window gets.
  const settingsPreview = useSettingsStore((s) => s.boardFilePreview)
  const seenPreview = useRef(settingsPreview)
  useEffect(() => {
    if (seenPreview.current === settingsPreview) return
    seenPreview.current = settingsPreview
    void load()
  }, [settingsPreview, load])

  const flipPreview = useCallback(async (): Promise<FlipResult> => {
    if (!data?.canChange || busy) return { ok: false, cancelled: true }
    setBusy('board-file-preview')
    let out: FlipResult = { ok: true }
    try {
      await setBoardFilePreview(!data.boardFilePreview, scope)
    } catch (err) {
      out = { ok: false, cancelled: false, err }
    } finally {
      setBusy(null)
    }
    await load()
    return out
  }, [busy, data, load, scope])

  const flip = useCallback(
    async (core: DocCore): Promise<FlipResult> => {
      if (!data?.canChange || busy) return { ok: false, cancelled: true }
      const next = !core.on
      if (!next && core.open.files > 0) {
        const yes = await useConfirmDialogStore.getState().confirm({
          title: turnOffTitle(core),
          message: turnOffMessage(core.open),
          confirmLabel: 'Turn off',
          destructive: true,
        })
        if (!yes) return { ok: false, cancelled: true }
      }
      setBusy(core.id)
      let out: FlipResult = { ok: true }
      try {
        await setDocCore(core.id, next, scope)
      } catch (err) {
        out = { ok: false, cancelled: false, err }
      } finally {
        setBusy(null)
      }
      await load()
      return out
    },
    [busy, data, load, scope],
  )

  const later = data?.later ?? []
  const fail = loadError ? readFailure(loadError) : null

  const coresState = data ? (
    data.cores.length === 0 ? (
      <SettingsLoadState kind="empty" title="No kinds of files to choose yet" testId="multiplayer-empty">
        This server doesn&apos;t offer multiplayer for any kind of file yet.
      </SettingsLoadState>
    ) : undefined
  ) : fail ? (
    <SettingsLoadState
      kind="error"
      title="Couldn’t load the multiplayer settings"
      detail={fail.detail}
      testId="multiplayer-error"
      action={
        <Button disabled={retrying} onClick={retry}>
          {retrying ? 'Trying…' : 'Try again'}
        </Button>
      }
    >
      K2 couldn&apos;t read which kinds of files open in multiplayer on this server.
      {fail.shape ? <span className="block mt-1 text-[var(--color-text-muted)]">{shapeLine(fail.shape)}</span> : null}
    </SettingsLoadState>
  ) : (
    <SettingsLoadState kind="loading" title="Loading the multiplayer settings…" testId="multiplayer-loading" />
  )

  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-settings-id="multiplayer.cores" data-testid="multiplayer-section">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Multiplayer</h2>
        <p className={SETTING_ROW_HINT}>{MULTIPLAYER_INTRO}</p>
        {data && !data.canChange ? (
          <p className={`${SETTING_ROW_HINT} mt-2`} data-testid="multiplayer-member-line">
            {MULTIPLAYER_MEMBER_LINE}
          </p>
        ) : null}
      </header>
      {data && fail ? (
        <Callout tone="warn" role="alert" className="type-hint flex items-center justify-between gap-4" data-testid="multiplayer-refresh-error">
          <span>Couldn&apos;t refresh these switches, so they may be out of date.</span>
          <Button disabled={retrying} onClick={retry}>
            {retrying ? 'Trying…' : 'Try again'}
          </Button>
        </Callout>
      ) : null}
      <SettingsGroup
        title="Kinds of files"
        hint="When one is on, its files open in the multiplayer view, with the workspace's agent beside them."
        card
        testId="multiplayer-cores"
        state={coresState}
      >
        {(data?.cores ?? []).map((core) => (
          <CoreRow key={core.id} core={core} canChange={data?.canChange === true} busy={busy === core.id} onFlip={() => flip(core)} />
        ))}
      </SettingsGroup>
      {data && data.boardFilePreview !== null ? (
        <SettingsGroup title="Board files" card testId="multiplayer-board-files">
          <PreviewRow on={data.boardFilePreview} canChange={data.canChange} busy={busy === 'board-file-preview'} onFlip={flipPreview} />
        </SettingsGroup>
      ) : null}
      <EditorPreviewSwitch />
      {data?.canChange ? <LivePreviewSwitches /> : null}
      {data && later.length > 0 ? (
        <p className={SETTING_ROW_HINT} data-testid="multiplayer-later">
          Coming later: {later.join(', ')}.
        </p>
      ) : null}
    </div>
  )
}

/** Beside a switch while its write is in flight. */
function SavingNote(): React.JSX.Element {
  return (
    <span role="status" data-testid="multiplayer-saving" className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)] whitespace-nowrap">
      <Spinner />
      Saving…
    </span>
  )
}

/** Run a flip and say what it did under the row. */
function useFlipNote(): { note: ReturnType<typeof useActionNote>['note']; run: (flip: () => Promise<FlipResult>, ok: string, fail: string) => Promise<void> } {
  const { note, clear, done, failed } = useActionNote()
  const run = useCallback(
    async (flip: () => Promise<FlipResult>, ok: string, fail: string) => {
      clear()
      const r = await flip()
      if (r.ok) done(ok)
      else if (!r.cancelled) failed(fail, r.err)
    },
    [clear, done, failed],
  )
  return { note, run }
}

function PreviewRow({
  on,
  canChange,
  busy,
  onFlip,
}: {
  on: boolean
  canChange: boolean
  busy: boolean
  onFlip: () => Promise<FlipResult>
}): React.JSX.Element {
  const { note, run } = useFlipNote()
  return (
    <SettingSwitchRow
      settingId="multiplayer.board-file-preview"
      control={
        <>
        {busy ? <SavingNote /> : null}
        <Toggle
          checked={on}
          onChange={() =>
            void run(
              onFlip,
              on ? 'Off. Board files show a card.' : 'On. Board files open in a Board tab.',
              on ? 'Couldn’t turn this off. Board files still open in a tab.' : 'Couldn’t turn this on. Board files still show a card.',
            )
          }
          aria-label={BOARD_FILE_PREVIEW_LABEL}
          aria-busy={busy || undefined}
          disabled={!canChange || busy}
        />
        </>
      }
    >
      <div className="type-row text-[var(--color-text-primary)] flex items-center gap-2">
        <span>{BOARD_FILE_PREVIEW_NAME}</span>
        <BetaTag />
      </div>
      <div className={SETTING_ROW_HINT}>{BOARD_FILE_PREVIEW_LINE}</div>
      {note ? (
        <div className="mt-2">
          <ActionNote note={note} />
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}

function CoreRow({
  core,
  canChange,
  busy,
  onFlip,
}: {
  core: DocCore
  canChange: boolean
  busy: boolean
  onFlip: () => Promise<FlipResult>
}): React.JSX.Element {
  const { note, run } = useFlipNote()
  const failed = !core.on && core.lastChange && !core.lastChange.on ? core.lastChange.failed : []
  const plain = plainCore(core)
  const kind = plain.sentence
  const garden = core.view === 'garden'
  const okLine = garden
    ? core.on
      ? 'Boards are off. Your boards are kept.'
      : 'Boards are on for everyone on this server.'
    : core.on
      ? `Multiplayer is off for ${kind}. They open the usual way.`
      : `Multiplayer is on for ${kind}, for everyone on this server.`
  const failLine = garden
    ? core.on
      ? 'Couldn’t turn off boards. They’re still on.'
      : 'Couldn’t turn on boards. They’re still off.'
    : core.on
      ? `Couldn’t turn off multiplayer for ${kind}. It’s still on.`
      : `Couldn’t turn on multiplayer for ${kind}. It’s still off.`
  return (
    <SettingSwitchRow
      settingId={`multiplayer.${core.id}`}
      control={
        <>
          {busy ? <SavingNote /> : null}
          <Toggle checked={core.on} onChange={() => void run(onFlip, okLine, failLine)} aria-label={plain.name} aria-busy={busy || undefined} disabled={!canChange || busy} />
        </>
      }
    >
      <div className="type-row text-[var(--color-text-primary)] flex items-center gap-2">
        <span>{plain.name}</span>
        {core.status === 'beta' ? <BetaTag /> : null}
      </div>
      <div className={SETTING_ROW_HINT}>{plain.description}</div>
      {core.on ? (
        <div className={SETTING_ROW_HINT}>{core.view === 'garden' ? MULTIPLAYER_GARDEN_BETA_LINE : MULTIPLAYER_BETA_LINE}</div>
      ) : null}
      {core.on && core.open.files > 0 ? (
        <div className="type-hint text-[var(--color-text-primary)] mt-1" data-testid={`multiplayer-open-${core.id}`}>
          {openNowLine(core.open)}
        </div>
      ) : null}
      {failed.map((f) => (
        <div
          key={f.path}
          className="type-hint text-[var(--color-status-warn-text)] mt-1 leading-relaxed max-w-[75ch]"
          data-testid={`multiplayer-failed-${core.id}`}
        >
          {failedWriteBackLine(core, f)}
        </div>
      ))}
      {note ? (
        <div className="mt-2">
          <ActionNote note={note} testId={`multiplayer-note-${core.id}`} />
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}
