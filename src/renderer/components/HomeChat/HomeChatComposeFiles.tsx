// Files in a Home DM or channel's message box (contract-home-chat-attachments-v1;
// the server lists `home-chat-attachments-v1`).
//
// The agent chat's attach, the same pieces (Rosson 2026-11-23: identical to
// what we have elsewhere): its "+" (`ComposeAttachButton`) with its menu
// (Attach a file or image, Save to stash), its chips
// (`ComposeAttachmentChip`: a picture for an image, the name, ×) and the
// same ways in: the "+" (the native picker on the desktop, the browser's
// file input on the web client), a drop on the chat (Finder drops through
// the window's drop router, `local-file-drop-targets`, which the view
// registers; HTML5 drops on the web), and a paste of files into the field. The files are this
// computer's, whichever server holds the chat.
//
// What differs is only where the bytes go: each file uploads at once, in
// ranges, to the chat's server (`uploads.ts`), its chip showing how far it
// is; Send waits until all are up, then the message names their upload
// ids. Removing a chip stops its upload (and aborts it on the server).

import { useCallback, useEffect, useRef, useState, type ClipboardEvent, type DragEvent, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { composeAttachPlan, pickLocalComposeFiles } from '@/lib/pick-compose-files'
import { abortHomeChatUpload, type HomeChatFileLimits } from '@/lib/home-chat/api'
import {
  UploadAbortedError,
  fileUploadSource,
  isPreviewableImage,
  localPathUploadSource,
  uploadHomeChatFile,
  type HomeChatUploadSource,
} from '@/lib/home-chat/uploads'
import { ComposeAttachmentChip } from '@/components/Thread/ComposeAttach'
import { ThreadAttachmentChips } from '@/components/Thread/ThreadZenKit'
import { filesOfClipboard } from '@/components/Feedback/TicketComposeFiles'
import { formatFileSize as formatBytes } from '@/components/Thread/ThreadFileObjects'

/** A picture on a path is read whole for its chip up to this size. */
const PATH_PREVIEW_MAX = 10 << 20
/** Uploads at once (Sew takes 5 in flight per person). */
const AT_ONCE = 3

export interface HomeChatComposeFile {
  key: string
  name: string
  size: number
  image: boolean
  /** The picture for the chip (a File, or a small file read whole). */
  blob?: Blob
  /** 0..1 while it uploads; 1 when it is up. */
  progress: number
  /** The server's upload id, once made. */
  id: string | null
  done: boolean
}

export interface HomeChatComposeFilesApi {
  files: HomeChatComposeFile[]
  /** Some file is still on its way: Send waits. */
  uploading: boolean
  /** The finished uploads, in order, for `send`. */
  ids: () => string[]
  pick: () => void
  remove: (key: string) => void
  /** After a send: the chips go (their uploads are on the message). */
  clear: () => void
  onPaste: (e: ClipboardEvent<HTMLTextAreaElement>) => void
  onDropFiles: (e: DragEvent<HTMLDivElement>) => void
  /** Paths on this computer (a Finder drop the router handed the chat). */
  addPaths: (paths: string[]) => Promise<void>
  /** The web client's file input (render it once). */
  fileInput: JSX.Element
}

interface Running {
  ctl: AbortController
  id: string | null
}

let seq = 0

/** The files of one conversation's box. `conv` null or `on` false: nothing
 *  is taken. Errors (too large, too many, an upload that failed) go to
 *  `onError`. */
export function useHomeChatComposeFiles({
  scope,
  conv,
  on,
  limits,
  onError,
}: {
  scope: ServerScope
  conv: string | null
  on: boolean
  limits: HomeChatFileLimits
  onError: (message: string) => void
}): HomeChatComposeFilesApi {
  const [files, setFiles] = useState<HomeChatComposeFile[]>([])
  const filesRef = useRef(files)
  filesRef.current = files
  const running = useRef(new Map<string, Running>())
  const queue = useRef<Array<{ key: string; src: HomeChatUploadSource }>>([])
  const errRef = useRef(onError)
  errRef.current = onError
  const ctx = useRef({ scope, conv })
  ctx.current = { scope, conv }

  const patch = useCallback((key: string, p: Partial<HomeChatComposeFile>) => {
    setFiles((cur) => cur.map((f) => (f.key === key ? { ...f, ...p } : f)))
  }, [])

  const drop = useCallback((key: string) => {
    setFiles((cur) => cur.filter((f) => f.key !== key))
  }, [])

  // Start what waits, AT_ONCE at a time.
  const pump = useCallback(() => {
    while (running.current.size < AT_ONCE && queue.current.length > 0) {
      const next = queue.current.shift()
      if (!next) break
      const { key, src } = next
      const { scope: sc, conv: cv } = ctx.current
      if (!cv) continue
      const run: Running = { ctl: new AbortController(), id: null }
      running.current.set(key, run)
      void uploadHomeChatFile(sc, cv, src, {
        signal: run.ctl.signal,
        onCreated: (id) => {
          run.id = id
          patch(key, { id })
        },
        onProgress: (ratio) => patch(key, { progress: Math.min(1, ratio) }),
      })
        .then((id) => patch(key, { id, progress: 1, done: true }))
        .catch((err: unknown) => {
          if (err instanceof UploadAbortedError) return
          drop(key)
          errRef.current(`Couldn't upload ${src.name}: ${err instanceof Error ? err.message : String(err)}`)
        })
        .finally(() => {
          running.current.delete(key)
          pump()
        })
    }
  }, [drop, patch])

  const take = useCallback(
    (sources: HomeChatUploadSource[], previews: Array<Blob | undefined>) => {
      if (!on || !ctx.current.conv) return
      const room = limits.perMessage - filesRef.current.length
      if (sources.length > room) errRef.current(`A message takes at most ${limits.perMessage} files.`)
      const added: HomeChatComposeFile[] = []
      sources.slice(0, Math.max(0, room)).forEach((src, i) => {
        if (src.size <= 0) {
          errRef.current(`${src.name} is empty: there is nothing to attach.`)
          return
        }
        if (limits.maxBytes !== null && src.size > limits.maxBytes) {
          errRef.current(`${src.name} is larger than this server takes (${formatBytes(limits.maxBytes)}).`)
          return
        }
        seq += 1
        const key = `f${seq}`
        added.push({ key, name: src.name, size: src.size, image: isPreviewableImage(src.name, src.type), blob: previews[i], progress: 0, id: null, done: false })
        queue.current.push({ key, src })
      })
      if (added.length === 0) return
      setFiles((cur) => [...cur, ...added])
      pump()
    },
    [limits.maxBytes, limits.perMessage, on, pump],
  )

  const addBrowserFiles = useCallback((list: File[]) => take(list.map(fileUploadSource), list), [take])

  const addPaths = useCallback(
    async (paths: string[]) => {
      try {
        const sources = await Promise.all(paths.map(localPathUploadSource))
        const previews = await Promise.all(
          sources.map(async (s) =>
            isPreviewableImage(s.name, s.type) && s.size <= PATH_PREVIEW_MAX ? new Blob([(await s.read(0, s.size)) as BlobPart], { type: s.type }) : undefined,
          ),
        )
        take(sources, previews)
      } catch (err) {
        errRef.current(`Couldn't attach: ${err instanceof Error ? err.message : String(err)}`)
      }
    },
    [take],
  )

  const stop = useCallback((key: string) => {
    queue.current = queue.current.filter((q) => q.key !== key)
    const run = running.current.get(key)
    run?.ctl.abort()
    const id = run?.id ?? filesRef.current.find((f) => f.key === key)?.id ?? null
    if (id) void abortHomeChatUpload(ctx.current.scope, id).catch(() => undefined)
  }, [])

  const remove = useCallback(
    (key: string) => {
      const f = filesRef.current.find((x) => x.key === key)
      if (f && !f.done) stop(key)
      else if (f?.id) void abortHomeChatUpload(ctx.current.scope, f.id).catch(() => undefined)
      drop(key)
    },
    [drop, stop],
  )

  const clear = useCallback(() => {
    queue.current = []
    setFiles([])
  }, [])

  // Another conversation (or the box going away): what was on its way stops.
  useEffect(() => {
    const runningNow = running.current
    return () => {
      for (const f of filesRef.current) if (!f.done) stop(f.key)
      queue.current = []
      runningNow.clear()
      setFiles([])
    }
  }, [conv, stop])

  const inputRef = useRef<HTMLInputElement | null>(null)
  const pick = useCallback(() => {
    if (!on) return
    // The person's own files, on this computer, whichever server holds the chat.
    const plan = composeAttachPlan({ activeHost: 'local' })
    if (plan.kind === 'native') {
      void pickLocalComposeFiles()
        .then((paths) => {
          if (paths && paths.length > 0) return addPaths(paths)
        })
        .catch((err: unknown) => errRef.current(`Couldn't attach: ${err instanceof Error ? err.message : String(err)}`))
      return
    }
    inputRef.current?.click()
  }, [addPaths, on])

  const fromList = useCallback(
    (list: File[]) => {
      const paths: string[] = []
      const browser: File[] = []
      for (const f of list) {
        const p = (f as unknown as { path?: string }).path
        if (p) paths.push(p)
        else browser.push(f)
      }
      if (paths.length > 0) void addPaths(paths)
      if (browser.length > 0) addBrowserFiles(browser)
    },
    [addBrowserFiles, addPaths],
  )

  const onPaste = useCallback(
    (e: ClipboardEvent<HTMLTextAreaElement>) => {
      if (!on) return
      const pasted = filesOfClipboard(e.clipboardData)
      if (pasted.length === 0) return // text pastes as text
      e.preventDefault()
      fromList(pasted)
    },
    [fromList, on],
  )
  const onDropFiles = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      if (!on) return
      const list = e.dataTransfer?.files ? Array.from(e.dataTransfer.files) : []
      if (list.length > 0) fromList(list)
    },
    [fromList, on],
  )

  const fileInput = (
    <input
      ref={inputRef}
      type="file"
      multiple
      hidden
      data-testid="home-chat-file-input"
      onChange={(e) => {
        const list = e.target.files ? Array.from(e.target.files) : []
        e.target.value = ''
        if (list.length > 0) fromList(list)
      }}
    />
  )

  const ids = useCallback(() => filesRef.current.filter((f) => f.done && f.id).map((f) => f.id as string), [])
  const uploading = files.some((f) => !f.done)
  return { files, uploading, ids, pick, remove, clear, onPaste, onDropFiles, addPaths, fileInput }
}

/** The chips above the box: the agent chat's, with each upload's progress. */
export function HomeChatComposeFileChips({
  files,
  onRemove,
}: {
  files: readonly HomeChatComposeFile[]
  onRemove: (key: string) => void
}): JSX.Element | null {
  if (files.length === 0) return null
  return (
    <ThreadAttachmentChips testId="home-chat-compose-files">
      {files.map((f) => (
        <ComposeAttachmentChip
          key={f.key}
          name={f.name}
          title={`${f.name} · ${formatBytes(f.size)}`}
          image={f.image}
          blob={f.blob}
          onRemove={() => onRemove(f.key)}
          progress={f.done ? null : f.progress}
        />
      ))}
    </ThreadAttachmentChips>
  )
}
