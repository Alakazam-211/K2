// Files a person attaches in a ticket's message box (0.46.6,
// prd-ticket-attachments-v1 TK20; daemon feature `ticket-attachments-v1`).
//
// The Thread compose bar's attach, for the ticket rail: "+" picks (the
// native picker on this computer, the browser's file input on the web
// client, the server's file picker when the window is on another K2), a
// drop on the box and a paste of files into the field all become chips
// above the box (at most 10, × to remove). Each resolves through
// `resolveThreadComposeFiles` against the TICKET's workspace: a file already
// inside it on this computer is attached where it is; anything else is
// uploaded into `<workspace>/.k2/downloads/` first. A file dragged from the
// Files drawer, or dropped by the window's drop router, arrives as
// `k2so:compose-insert` on the box (`data-compose-bar`), as on the Thread.
//
// The chips persist beside the text draft (`composer-drafts`, per ticket)
// and ride the next message (`comment {…, attachments:[{path}]}`), an
// option pick included. Off, with the reason on the "+", when the ticket's
// workspace is gone (unlinked) or the server doesn't take ticket files.

import { useCallback, useEffect, useRef, useState, type ClipboardEvent, type DragEvent, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { ComposeInsertDetail } from '@/lib/external-drop-router'
import { getTicketDraftFiles, setTicketDraftFiles } from '@/lib/composer-drafts'
import { composeAttachPlan, pickLocalComposeFiles, pickRemoteComposeFile } from '@/lib/pick-compose-files'
import {
  THREAD_POST_TOO_MANY_FILES,
  isInsideWorkspace,
  mergeThreadComposeFiles,
  resolveThreadComposeFiles,
  threadComposeFile,
  type ThreadComposeAttachInput,
  type ThreadComposeFile,
} from '@/lib/thread-compose-attachments'
import { ThreadAttachmentChip, ThreadAttachmentChips, ThreadRoundButton } from '@/components/Thread/ThreadZenKit'
import { TICKET_ATTACHMENTS_FEATURE } from './feedback-api'

/** May this ticket's box take files, and if not, why (the "+" title). */
export function ticketComposeAttach(
  scope: Pick<ServerScope, 'serverSupports'>,
  projectPath: string | null | undefined,
): { on: boolean; why: string | null } {
  if (!scope.serverSupports(TICKET_ATTACHMENTS_FEATURE)) {
    return { on: false, why: "This server's K2 doesn't take files on tickets yet (it needs 0.46.6)." }
  }
  if (!projectPath) return { on: false, why: "This ticket's workspace was removed, so files can't be attached." }
  return { on: true, why: null }
}

/** Browser files on a clipboard (a pasted screenshot). */
export function filesOfClipboard(data: Pick<DataTransfer, 'files' | 'items'> | null | undefined): File[] {
  if (!data) return []
  const out: File[] = []
  if (data.files && data.files.length > 0) {
    for (let i = 0; i < data.files.length; i++) out.push(data.files[i])
    return out
  }
  const items = data.items
  if (!items) return out
  for (let i = 0; i < items.length; i++) {
    const it = items[i]
    if (it.kind !== 'file') continue
    const f = it.getAsFile()
    if (f) out.push(f)
  }
  return out
}

/** Split dropped files into paths on this computer and browser files. */
function splitDropped(list: FileList | File[]): { paths: string[]; files: File[] } {
  const paths: string[] = []
  const files: File[] = []
  for (let i = 0; i < list.length; i++) {
    const f = list[i]
    const p = (f as unknown as { path?: string }).path
    if (p) paths.push(p)
    else files.push(f)
  }
  return { paths, files }
}

export interface TicketComposeFilesApi {
  files: ThreadComposeFile[]
  /** Attach picked/dropped/pasted files (`onServer`: already on the
   *  ticket's server, e.g. the server picker or an upload the router did). */
  add: (input: ThreadComposeAttachInput & { onServer?: boolean }) => Promise<void>
  remove: (path: string) => void
  /** After a send: no chips (and no saved ones). */
  clear: () => void
  pick: () => void
  onPaste: (e: ClipboardEvent<HTMLTextAreaElement>) => void
  onDropFiles: (e: DragEvent<HTMLDivElement>) => void
  /** The box's root: `k2so:compose-insert` arrives here. */
  rootRef: (el: HTMLDivElement | null) => void
  /** The web client's file input (render it once, hidden). */
  fileInput: JSX.Element
}

/** The chips of one ticket's box. `on` false: nothing is taken. Errors
 *  (an upload failed, too many files) go to `onError`. */
export function useTicketComposeFiles({
  ticketId,
  scope,
  workspacePath,
  on,
  onError,
}: {
  ticketId: string
  scope: ServerScope
  workspacePath: string | null
  on: boolean
  onError: (message: string) => void
}): TicketComposeFilesApi {
  // The saved chips load whatever `on` says (it is false until the ticket
  // has loaded); `on` only gates taking new files and what the rail draws.
  const [files, setFiles] = useState<ThreadComposeFile[]>(() => getTicketDraftFiles(ticketId))
  const errRef = useRef(onError)
  errRef.current = onError
  const loadedFor = useRef(ticketId)
  useEffect(() => {
    if (loadedFor.current === ticketId) return
    loadedFor.current = ticketId
    setFiles(getTicketDraftFiles(ticketId))
  }, [ticketId])
  useEffect(() => {
    if (loadedFor.current !== ticketId) return
    setTicketDraftFiles(ticketId, files)
  }, [files, ticketId])

  const add = useCallback(
    async (input: ThreadComposeAttachInput & { onServer?: boolean }) => {
      if (!on) return
      const ws = workspacePath ?? undefined
      let got: ThreadComposeFile[] | null
      try {
        if (input.onServer) {
          // Already on the ticket's server: attach the ones inside its
          // workspace; the daemon refuses any other (outside_workspace).
          got = (input.paths ?? []).filter((p) => isInsideWorkspace(p, ws)).map(threadComposeFile)
          if ((input.paths ?? []).length > 0 && got.length === 0) {
            errRef.current("That file isn't inside this ticket's workspace.")
            return
          }
        } else {
          got = await resolveThreadComposeFiles(scope, { ...input, workspacePath: ws })
        }
      } catch (err) {
        errRef.current(`Couldn't attach: ${err instanceof Error ? err.message : String(err)}`)
        return
      }
      if (!got || got.length === 0) return
      const more = got
      setFiles((cur) => {
        const merged = mergeThreadComposeFiles(cur, more)
        if (merged.dropped > 0) errRef.current(THREAD_POST_TOO_MANY_FILES)
        return merged.files
      })
    },
    [on, scope, workspacePath],
  )
  const remove = useCallback((path: string) => setFiles((cur) => cur.filter((f) => f.path !== path)), [])
  const clear = useCallback(() => {
    setFiles([])
    setTicketDraftFiles(ticketId, [])
  }, [ticketId])

  const inputRef = useRef<HTMLInputElement | null>(null)
  const pick = useCallback(() => {
    if (!on) return
    const plan = composeAttachPlan()
    if (plan.kind === 'native') {
      void pickLocalComposeFiles()
        .then((paths) => {
          if (paths && paths.length > 0) return add({ paths })
        })
        .catch((err: unknown) => errRef.current(`Couldn't attach: ${err instanceof Error ? err.message : String(err)}`))
      return
    }
    if (plan.kind === 'web-input') {
      inputRef.current?.click()
      return
    }
    void pickRemoteComposeFile().then((path) => {
      if (path) return add({ paths: [path], onServer: true })
    })
  }, [add, on])

  const onPaste = useCallback(
    (e: ClipboardEvent<HTMLTextAreaElement>) => {
      if (!on) return
      const pasted = filesOfClipboard(e.clipboardData)
      if (pasted.length === 0) return // text pastes as text
      e.preventDefault()
      const { paths, files: browser } = splitDropped(pasted)
      void add(paths.length > 0 ? { paths } : { files: browser })
    },
    [add, on],
  )
  const onDropFiles = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      if (!on) return
      const { paths, files: browser } = splitDropped(e.dataTransfer.files)
      void add(paths.length > 0 ? { paths } : { files: browser })
    },
    [add, on],
  )

  // The drop router and the Files drawer's drag hand files to the box as
  // `k2so:compose-insert` (paths, `uploaded` when already on the server).
  const [root, setRoot] = useState<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!root || !on) return
    const onInsert = (e: Event): void => {
      const detail = (e as CustomEvent<ComposeInsertDetail>).detail
      if (detail?.paths && detail.paths.length > 0) void add({ paths: detail.paths, onServer: detail.uploaded === true })
    }
    root.addEventListener('k2so:compose-insert', onInsert)
    return () => root.removeEventListener('k2so:compose-insert', onInsert)
  }, [root, on, add])

  const fileInput = (
    <input
      ref={inputRef}
      type="file"
      multiple
      hidden
      data-testid="ticket-compose-file-input"
      onChange={(e) => {
        const list = e.target.files
        const picked = list ? Array.from(list) : []
        e.target.value = ''
        if (picked.length === 0) return
        const { paths, files: browser } = splitDropped(picked)
        void add(paths.length > 0 ? { paths } : { files: browser })
      }}
    />
  )

  return { files, add, remove, clear, pick, onPaste, onDropFiles, rootRef: setRoot, fileInput }
}

/** The "+" in the ticket's box; off with the reason as its title. */
export function TicketAttachButton({
  on,
  why,
  busy,
  onClick,
}: {
  on: boolean
  why: string | null
  busy: boolean
  onClick: () => void
}): JSX.Element {
  return (
    <ThreadRoundButton
      label="Attach files"
      title={on ? 'Attach files (or drop or paste them here)' : why ?? 'Files can’t be attached here'}
      disabled={!on || busy}
      lightable
      onClick={onClick}
      data={{ 'data-testid': 'ticket-compose-attach', 'data-ticket-attach-off': on ? undefined : '' }}
    >
      +
    </ThreadRoundButton>
  )
}

/** The chips above the box. */
export function TicketComposeFileChips({
  files,
  onRemove,
}: {
  files: readonly ThreadComposeFile[]
  onRemove: (path: string) => void
}): JSX.Element | null {
  if (files.length === 0) return null
  return (
    <ThreadAttachmentChips testId="ticket-compose-files">
      {files.map((f) => (
        <ThreadAttachmentChip
          key={f.path}
          name={f.name}
          title={f.path}
          onRemove={() => onRemove(f.path)}
        />
      ))}
    </ThreadAttachmentChips>
  )
}
