// prd-zen-user-widgets-v2 S3 (UW13, UW15, UW30, UW38, UW39, UWA5, UWB13)
// — the sealed frame of one custom widget.
//
// 1. Fetch the bundle (`GET widget/bundle`) for the payload's `hash`, and
//    the widget's libraries (`zenLib.texts`, B3), all or nothing.
// 2. Draw it through the one `HtmlFrame`, profile `widget`: K2's prelude
//    (theme style, the nonced `k2-frame.js`, the nonced libraries) comes
//    before any of the widget's HTML, under the nonce CSP.
// 3. On the frame's `load`: the private port and K2's hello
//    (`startZenFrameHost`), with the custom layer (`createZenCustomLayer`)
//    checking every message against the widget's caps, reach and budgets.
// A problem loading (an older daemon, no good bundle, a library that can't
// be served) goes to the parent, which draws K2's card instead. The frame
// is remounted (keyed by the parent) on a new hash or a Reload.

import { useContext, useEffect, useMemo, useRef, useState } from 'react'
import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameHello } from '@/lib/zen/zen-custom-types'
import {
  fetchZenWidgetBundle,
  pauseZenWidget,
  reportZenWidgetRun,
  zenRunReportBody,
  zenWidgetError,
  type ZenWidgetErrorCode,
} from '@/lib/zen/zen-widget-routes'
import { zenLib, ZenLibError } from '@/lib/zen/zen-lib-loader'
import { zenFramePrelude, type ZenFrameTheme } from '@/lib/zen/zen-custom-prelude'
import {
  ZEN_PICK_GESTURE_MS,
  ZenCustomRefusal,
  createZenCustomLayer,
  type ZenCustomLayer,
  type ZenCustomUploads,
  type ZenUploadedFile,
} from '@/lib/zen/zen-custom-bridge'
import { ZenUploadsError, zenPick } from '@/lib/zen/zen-custom-uploads'
import { registerZenDropTarget } from '@/lib/zen/zen-drop-targets'
import { pickLocalComposeFiles } from '@/lib/pick-compose-files'
import { pickBrowserFiles } from '@/lib/zen/zen-thread-files'
import { isWebClient } from '@/lib/is-web'
import { useToastStore } from '@/stores/toast'
import { zenCustomSourcesFor } from '@/lib/zen/zen-custom-sources'
import { createZenCustomStore, zenDataSpace } from '@/lib/zen/zen-custom-store'
import { createZenCustomGuests, type ZenGuestRoomsAsk } from '@/lib/zen/zen-custom-guests'
import { createZenCustomWork, type ZenWorkChange } from '@/lib/zen/zen-custom-work'
import { ZenWorkSheet } from './ZenWorkSheet'
import { ZenGuestRoomsSheet } from './ZenGuestRoomsSheet'
import { createZenPermissionSheet, type ZenPermissionAsk } from '@/lib/zen/permission-sheet'
import { ZenPermissionSheet } from './ZenPermissionSheet'
import { allowZenLinkSite, createZenLinks, fetchZenLinkSites, type ZenLinkAnswer, type ZenLinkAsk } from '@/lib/zen/zen-links'
import { ZenLinkSheet } from './ZenLinkSheet'
import { ZenDownloadSheet } from './ZenDownloadSheet'
import { daemonZenDownloadConsent, spendZenDownload, zenDownloadConsent, type ZenDownloadAnswer, type ZenDownloadAsk } from '@/lib/zen/zen-download-consent'
import { openUrl } from '@tauri-apps/plugin-opener'
import { gardenBlobGet, gardenBlobIngest, gardenBlobPut, gardenCall, type GardenBlobPlace } from '@/lib/garden-data/garden-data-api'
import { createZenCustomBlobs } from '@/lib/zen/zen-custom-blobs'
import { downloadChunks } from '@/lib/fs-transfer'
import type { GardenCallBody } from '@/lib/garden-data/garden-data-types'
import { onGardenDataChanged } from '@/stores/session-events'
import { startZenFrameHost } from '@/lib/zen/zen-custom-host'
import { ZenColumnsContext } from '@/lib/zen/zen-columns'
import {
  isZenWidgetPostingPaused,
  pauseZenWidgetPosting,
  reloadZenWidget,
  stopZenWidget,
  zenPlacementKey,
} from '@/lib/zen/zen-custom-run'
import { ensureZenConversation, zenAgentRows, zenCustomViewFor, zenOpenAgentTerminal, zenThreadItemTarget } from '@/lib/zen/zen-data'
import { zenWidgetDisplayName } from '@/lib/zen/zen-custom-words'
import { createZenChordGate, zenFrameHasFocus, type ZenChordActions } from '@/lib/zen/zen-shortcut'
import { currentZenGardenId, switchZenGardenByIndex, useZenGardensStore } from '@/lib/zen/zen-gardens'
import { cycleZenTheme } from '@/lib/zen/zen-theme-switch'
import { useZenConfigStore, zenLocalScope } from '@/lib/zen/zen-api'
import { exitZen } from '@/lib/zen/zen-view'
import { registerZenCanvasFrame, setZenCanvasRegions, startZenCanvasDrag, zenCanvasPointer, zenCanvasPressedWithin } from '@/lib/zen/zen-canvas'

/** UW33: one gate for every widget frame in this window (they share the
 *  500 ms budget), acting as K2's own keys would. */
const zenFrameChordGate = createZenChordGate()

/** Garden v3: a widget asking for this reports errors without text (GV49). */
const SOURCES_READ = 'sources:read'

const ZEN_FRAME_CHORD_ACTIONS: ZenChordActions = {
  exitZen: () => exitZen(),
  switchGarden: (n) => switchZenGardenByIndex(n - 1),
  cycleTheme: (dir) => {
    const gardenId = currentZenGardenId()
    if (!gardenId) return
    const scope = useZenConfigStore.getState().page?.themeScope ?? 'global'
    // The daemon's zen_changed re-reads the page with the new theme.
    void cycleZenTheme(dir, scope, gardenId).catch((err: unknown) => console.warn('[zen] theme from a widget key failed:', err))
  },
}

/** Why the bundle couldn't be shown (the parent draws K2's card). */
export interface ZenCustomLoadProblem {
  code: ZenWidgetErrorCode
  message: string
}

/** The `/boot-status` features the widget rows need: this daemon served the
 *  bundle, so it has them (UW36). B1's generated table names each row's. */
export const ZEN_FRAME_FEATURES: readonly string[] = [
  'zen-v1',
  'zen-gardens-v1',
  'zen-widgets-v1',
  'sources-v1',
  'garden-data-v1',
  // RS-A15 `guests.rooms` / `guests.setRooms` (0.46.1).
  'guests-rooms-v1',
  // 0.46.3 Thread parity: `thread.markRead`, `thread.attachment`,
  // `thread.open`, `thread.links`, `thread.pickFiles` (each still checks the
  // agent’s server and answers verb_unavailable on an older one; no ASCII
  // apostrophe here: zen-frame-shape.test.ts reads this list by its quotes).
  'thread-read-v1',
  'thread-attachments-v1',
  'thread-images-v1',
  // 0.46.5 activity SDK: `thread.onActivity` (the renderer answers it from
  // the Thread feed; an agent server without turns gives source none).
  'widget-activity-v1',
  // 0.46.5 (prd-garden-board-needs-v1): blobs, and k2.download / k2.pick /
  // k2.onDrop.
  'garden-blobs-v1',
  'zen-files-v1',
  // 0.46.6: store.history, store.restore and store.snapshot (the daemon answers them).
  'garden-history-v1',
  // 0.46.7 Workbench (catalog v40): work.list, git.*, compute.jobs, the
  // sheet-gated start / land / settle, git.note and agents.openTerminal.
  'workbench-v1',
]

/** The Garden's theme as the frame gets it: the Zen root's `--zen-*`
 *  variables and scheme (UW39), and reduced motion. */
export function readZenFrameTheme(root: HTMLElement | null): ZenFrameTheme & { reduced: boolean } {
  const vars: Record<string, string> = {}
  if (root) {
    const s = root.style
    for (let i = 0; i < s.length; i++) {
      const name = s[i]
      if (name.startsWith('--zen-')) vars[name] = s.getPropertyValue(name).trim()
    }
  }
  const scheme = root?.getAttribute('data-zen-scheme') === 'dark' ? 'dark' : 'light'
  return { vars, scheme, reduced: root?.hasAttribute('data-zen-reduced-motion') ?? false }
}

/** `theme.get` / `theme.changed` (the catalog's ThemeInfo): `{theme:
 *  {scheme, vars}, chrome: {corners, stoplights}, motion: {reduced}}`. The
 *  window's `[chrome]` as the Zen root shows it, never free-form placement
 *  (UWA14). */
export function zenFrameThemeInfo(root: HTMLElement | null): {
  theme: { scheme: 'light' | 'dark'; vars: Record<string, string> }
  chrome: { corners: string | null; stoplights: string | null }
  motion: { reduced: boolean }
} {
  const t = readZenFrameTheme(root)
  return {
    theme: { scheme: t.scheme, vars: t.vars },
    chrome: { corners: root?.getAttribute('data-zen-corners') ?? null, stoplights: root?.getAttribute('data-zen-stoplights') ?? null },
    motion: { reduced: t.reduced },
  }
}

type Loaded = { html: string; nonce: string; prelude: string; hash: string }

export function ZenCustomFrame({
  widget,
  gardenId,
  bridge,
  onProblem,
}: {
  widget: ZenCustomWidgetPayload
  gardenId: string
  bridge: ZenWidgetBridge
  onProblem(problem: ZenCustomLoadProblem): void
}): React.JSX.Element {
  const [loaded, setLoaded] = useState<Loaded | null>(null)
  // RS-A15: K2's confirm sheet for a guests.setRooms write (outside the frame).
  const [guestAsk, setGuestAsk] = useState<{ ask: ZenGuestRoomsAsk; resolve: (token: string | null) => void } | null>(null)
  // Permission cards (0.46.3): K2's sheet for a card the widget shows.
  const [permissionAsk, setPermissionAsk] = useState<{ ask: ZenPermissionAsk; resolve: () => void } | null>(null)
  // `k2.open` (0.46.4): K2's card the first time a site is opened from this Garden.
  const [linkAsk, setLinkAsk] = useState<{ ask: ZenLinkAsk; resolve: (answer: ZenLinkAnswer) => void } | null>(null)
  // 0.46.7 Workbench: K2's sheet for a widget's Start, Land, Retire or Discard.
  const [workAsk, setWorkAsk] = useState<{ change: ZenWorkChange; resolve: (yes: boolean) => void } | null>(null)
  const [downloadAsk, setDownloadAsk] = useState<{ ask: ZenDownloadAsk; resolve: (answer: ZenDownloadAnswer) => void } | null>(null)
  const loadedRef = useRef<Loaded | null>(null)
  loadedRef.current = loaded
  const boxRef = useRef<HTMLDivElement | null>(null)
  const frameRef = useRef<HTMLIFrameElement | null>(null)
  const hostRef = useRef<{ dispose(): void } | null>(null)
  const layerRef = useRef<ZenCustomLayer | null>(null)
  const widgetRef = useRef(widget)
  widgetRef.current = widget
  // `k2.layout.*`: the columns of the Garden page this frame is drawn on.
  const columns = useContext(ZenColumnsContext)
  const columnsRef = useRef(columns)
  columnsRef.current = columns
  // 0.46.1: the placement's source addresses changed (a config edit, an
  // approval): start any `not_bound` subscription the layer is holding.
  const sourcesSig = (widget.sourceAddresses ?? []).join('\n')
  useEffect(() => {
    layerRef.current?.boundChanged()
  }, [sourcesSig])
  const problemRef = useRef(onProblem)
  problemRef.current = onProblem
  const key = zenPlacementKey(gardenId, widget.id)

  // 1. Bundle and libraries (once per mount: the parent keys us by hash).
  useEffect(() => {
    let live = true
    const root = boxRef.current?.closest<HTMLElement>('[data-zen-root]') ?? null
    // 0.46.5 WL7: exactly the daemon's resolved `libKeys`; an older daemon's
    // `libs` go through the same resolver in the loader.
    void Promise.all([fetchZenWidgetBundle(widget.widget, gardenId), zenLib.texts(widget.libKeys ?? widget.libs)])
      .then(([bundle, libs]) => {
        if (!live) return
        // The caps, sources and agent rows this frame gets come from the
        // page payload, so the code it runs must be the version that
        // payload describes. A bundle that changed since (a write between
        // the page and this fetch) never runs under the old version's
        // rights: nothing renders; the payload for the new version re-keys
        // this frame (security review finding 5).
        if (bundle.hash !== widget.hash) {
          problemRef.current({
            code: 'failed',
            message: 'This widget changed while its page was loading. It starts once K2 has read the new version.',
          })
          return
        }
        const prelude = zenFramePrelude({ nonce: bundle.nonce, theme: readZenFrameTheme(root), libs, ownScheme: widget.colorScheme === 'own' })
        setLoaded({ html: bundle.html, nonce: bundle.nonce, prelude, hash: bundle.hash })
      })
      .catch((err: unknown) => {
        if (!live) return
        if (err instanceof ZenLibError) {
          problemRef.current({ code: 'failed', message: `This widget needs a library K2 can’t load: ${err.message}` })
          return
        }
        const e = zenWidgetError(err)
        problemRef.current({ code: e.code, message: e.message })
      })
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => () => hostRef.current?.dispose(), [])

  // The whole canvas (`zen-canvas`): the frame's hit regions and window
  // drag are keyed by its placement; they go with the frame.
  useEffect(() => {
    const el = frameRef.current
    if (!loaded || !el) return undefined
    return registerZenCanvasFrame(key, el)
  }, [loaded, key])

  const label = useMemo(() => zenWidgetDisplayName(widget.name), [widget.name])

  // 3. The port and the hello, on load.
  const onLoad = (el: HTMLIFrameElement): void => {
    const win = el.contentWindow
    if (!win) return
    hostRef.current?.dispose()
    const root = boxRef.current?.closest<HTMLElement>('[data-zen-root]') ?? null
    const theme = readZenFrameTheme(root)
    const w = widgetRef.current
    const hello: ZenFrameHello = {
      k2: 'hello',
      v: 1,
      caps: [...w.caps],
      features: [...ZEN_FRAME_FEATURES],
      widget: { id: w.id, name: label, garden: gardenId },
      config: { ...w.props.config },
      motion: { reduced: theme.reduced },
    }
    hostRef.current = startZenFrameHost(win, {
      hello,
      label: `${w.widget} (${gardenId}/${w.id})`,
      stop: (reason) => stopZenWidget(key, reason),
      focused: () => zenFrameHasFocus(frameRef.current),
      onChord: (chord, frameFocused) => zenFrameChordGate.run(chord, frameFocused, Date.now(), ZEN_FRAME_CHORD_ACTIONS),
      onPointer: (down) => zenCanvasPointer(key, down),
      // Runtime errors reach agents (decision-k2-runtime-v1 stage 0 (d)):
      // logged by the daemon against the bundle hash this frame runs.
      // GV49: a widget that reads data sources reports fixed fields only.
      onReport: (r) => {
        const hash = loadedRef.current?.hash
        if (!hash) return
        const cur = widgetRef.current
        const readsSources =
          (cur.caps as readonly string[]).includes(SOURCES_READ) || (cur.requested as readonly string[]).includes(SOURCES_READ)
        void reportZenWidgetRun(zenRunReportBody(cur.widget, hash, r, readsSources)).catch((err: unknown) =>
          console.warn('[zen] the widget runtime report failed:', err),
        )
      },
      makeLayer: (push, pushError) => {
        const blobPlace = (): GardenBlobPlace => ({
          garden: gardenId,
          placement: widgetRef.current.id,
          hash: loadedRef.current?.hash ?? widgetRef.current.hash,
        })
        // prd-garden-board-needs-v1 GB12/GB13: this widget's files, over
        // the daemon's widget door for the placement and the hash this
        // frame runs (8 MiB chunks; the daemon runs the call gates).
        const blobs = createZenCustomBlobs({
          put: (chunk) => gardenBlobPut(blobPlace(), chunk),
          get: (params) => gardenBlobGet(blobPlace(), params),
        })
        // GP1–GP4 (0.46.6): K2's picker, and drops the router hands this
        // frame while its widget listens (k2.onDrop).
        const dropListeners = new Set<(d: { files: ZenUploadedFile[]; x: number; y: number }) => void>()
        let dropOff: (() => void) | null = null
        const uploads: ZenCustomUploads = {
          pressed: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS),
          pick: async (opts) => {
            try {
              return await zenPick(
                {
                  web: () => isWebClient(),
                  native: (o) => pickLocalComposeFiles(o),
                  browser: (o) => pickBrowserFiles(o),
                  ingest: (paths, accept) => gardenBlobIngest(blobPlace(), paths, accept).then((r) => r.files),
                  put: (bytes, o) => blobs.put(bytes, o),
                },
                opts,
              )
            } catch (err) {
              if (err instanceof ZenUploadsError) throw new ZenCustomRefusal(err.code, err.message)
              throw err
            }
          },
          onDrop: (fn) => {
            dropListeners.add(fn)
            const frame = frameRef.current
            if (!dropOff && frame) {
              dropOff = registerZenDropTarget(frame, (paths, at) => {
                void gardenBlobIngest(blobPlace(), paths.slice(0, 20), [])
                  .then((r) => {
                    for (const l of dropListeners) l({ files: r.files, x: at.x, y: at.y })
                  })
                  .catch((err: unknown) => {
                    console.warn('[zen] a drop on the widget failed:', err)
                    useToastStore.getState().addToast(`${label} didn’t take the dropped files: ${err instanceof Error ? err.message : String(err)}`, 'error')
                  })
              })
            }
            return () => {
              dropListeners.delete(fn)
              if (dropListeners.size === 0 && dropOff) {
                dropOff()
                dropOff = null
              }
            }
          },
        }
        const layer = createZenCustomLayer({
          widget: () => widgetRef.current,
          gardenId: () => gardenId,
          // The hash this frame runs: every Thread post names it with the
          // placement, for the daemon's own `thread:post` check (0.46.1).
          frameHash: () => loadedRef.current?.hash ?? widgetRef.current.hash,
          inner: bridge,
          boundRows: () => zenAgentRows(zenCustomViewFor(gardenId, widgetRef.current)),
          ensureConversation: (address) => ensureZenConversation(zenCustomViewFor(gardenId, widgetRef.current), address),
          push,
          pushError,
          sources: zenCustomSourcesFor(gardenId, () => widgetRef.current),
          // Garden data (contract-garden-data-v1 §7): this computer's daemon,
          // for the placement and the bundle hash this frame runs.
          store: createZenCustomStore({
            call: (verb, args) =>
              gardenCall({
                garden: gardenId,
                placement: widgetRef.current.id,
                hash: loadedRef.current?.hash ?? widgetRef.current.hash,
                verb,
                args,
              }).then((r) => r.result),
            onChanged: (cb) => onGardenDataChanged(zenLocalScope(), cb),
            space: zenDataSpace(widgetRef.current.widget),
            onStale: () => reloadZenWidget(key),
            setTimer: (fn, ms) => window.setTimeout(fn, ms),
            clearTimer: (t) => window.clearTimeout(t as number),
            now: () => Date.now(),
          }),
          blobs,
          uploads,
          // GS1–GS7: `k2.download` lands through K2's own download path
          // (transfer card, Cancel, toast; Downloads, never a folder the
          // widget names), only after a press in this frame within 5 s.
          downloads: {
            pressed: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS),
            // Security review #1: K2's card the first time this widget
            // saves a file in this Garden; "Always" is the daemon's (one
            // answer for every client).
            consent: ({ name, size }) =>
              zenDownloadConsent(
                daemonZenDownloadConsent,
                gardenId,
                widgetRef.current.widget,
                {
                  name,
                  size,
                  widget: label,
                  garden: useZenGardensStore.getState().gardens.find((g) => g.id === gardenId)?.name ?? 'this Garden',
                },
                (ask) => new Promise<ZenDownloadAnswer>((resolve) => setDownloadAsk({ ask, resolve })),
              ),
            // Review #1: the daemon's hourly budget, shared by every client.
            spend: (size) => spendZenDownload(gardenId, widgetRef.current.widget, size),
            // Review #4: a file a widget made is quarantined on macOS.
            save: (name, read) => downloadChunks(name, read, { quarantine: true }),
          },
          // RS-A15: local Apps' guests' rooms; a write waits for the
          // person on ZenGuestRoomsSheet (K2's DOM over the card).
          guests: createZenCustomGuests({
            call: (verb, args, confirmToken) =>
              gardenCall({
                garden: gardenId,
                placement: widgetRef.current.id,
                hash: loadedRef.current?.hash ?? widgetRef.current.hash,
                verb,
                args: args as GardenCallBody['args'],
                ...(confirmToken ? { confirmToken } : {}),
              }).then((r) => r.result),
            confirm: (ask) => new Promise((resolve) => setGuestAsk({ ask, resolve })),
          }),
          // 0.46.7 Workbench: the daemon's rows; a write only just after a
          // press in this frame, and start / land / settle wait for the
          // person on ZenWorkSheet (K2's DOM over the card).
          work: createZenCustomWork({
            call: (verb, args, confirmToken) =>
              gardenCall({
                garden: gardenId,
                placement: widgetRef.current.id,
                hash: loadedRef.current?.hash ?? widgetRef.current.hash,
                verb,
                args: args as GardenCallBody['args'],
                ...(confirmToken ? { confirmToken } : {}),
              }).then((r) => r.result),
            pressed: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS),
            confirm: (change) => new Promise<boolean>((resolve) => setWorkAsk({ change, resolve })),
          }),
          terminal: {
            pressed: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS),
            open: (address) => zenOpenAgentTerminal(zenCustomViewFor(gardenId, widgetRef.current), address),
          },
          // Permission cards: `thread.answer` on one opens K2's sheet over
          // the card (outside the frame), only after a press in the widget.
          permissions: createZenPermissionSheet({
            find: (address, cardId) => zenThreadItemTarget(address, cardId)?.item ?? null,
            show: (ask) => new Promise<void>((resolve) => setPermissionAsk({ ask, resolve })),
            gesture: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS) || zenFrameHasFocus(frameRef.current),
          }),
          // A handoff card is a person's: the bridge refuses a widget's answer.
          cardKind: (address, cardId) => zenThreadItemTarget(address, cardId)?.item.doc.kind ?? null,
          stop: (reason) => stopZenWidget(key, reason),
          // `thread.pickFiles` opens K2's picker only after the person
          // pressed in this widget a moment ago, or while it has the focus.
          gesture: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS) || zenFrameHasFocus(frameRef.current),
          paused: () => widgetRef.current.paused !== null || isZenWidgetPostingPaused(key),
          pauseForRunaway: () => {
            pauseZenWidgetPosting(key)
            void pauseZenWidget({ garden: gardenId, placement: widgetRef.current.id, reason: 'runaway' }).catch((err: unknown) =>
              console.warn('[zen] runaway: the daemon pause failed (this window stays paused):', err),
            )
          },
          theme: () => zenFrameThemeInfo(root),
          onThemeChange: (cb) => {
            if (!root || typeof MutationObserver === 'undefined') return () => undefined
            const mo = new MutationObserver(cb)
            mo.observe(root, {
              attributes: true,
              attributeFilter: ['style', 'data-zen-scheme', 'data-zen-reduced-motion', 'data-zen-corners', 'data-zen-stoplights'],
            })
            return () => mo.disconnect()
          },
          canvas: {
            setHitRegions: (rects) => setZenCanvasRegions(key, rects),
            startDrag: () => startZenCanvasDrag(key),
          },
          // Garden columns (A1, CA10): the page this frame is on, read live
          // (the controller is the page's for its whole life). A fold needs
          // the person's press in this frame within 5 s; focus alone isn't one.
          layout: columnsRef.current
            ? {
                columns: () => columnsRef.current?.columns() ?? [],
                setByWidget: (index, collapsed) => columnsRef.current?.setByWidget(index, collapsed) ?? 'bad_index',
                subscribe: (fn) => columnsRef.current?.subscribe(fn) ?? (() => undefined),
                pressed: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS),
              }
            : undefined,
          // `k2.open` (0.46.4): only after a press in this frame within 5 s
          // (focus alone isn't one); K2's card the first time per site in
          // this Garden; the person's browser.
          links: {
            pressed: () => zenCanvasPressedWithin(key, ZEN_PICK_GESTURE_MS),
            open: createZenLinks({
              gardenId: () => gardenId,
              sites: fetchZenLinkSites,
              allow: allowZenLinkSite,
              ask: (ask) => new Promise<ZenLinkAnswer>((resolve) => setLinkAsk({ ask, resolve })),
              describe: () => ({
                widget: label,
                garden: useZenGardensStore.getState().gardens.find((g) => g.id === gardenId)?.name ?? 'this Garden',
              }),
              openUrl: (url) => openUrl(url),
            }).open,
          },
          setTimer: (fn, ms) => window.setTimeout(fn, ms),
          clearTimer: (t) => window.clearTimeout(t as number),
          now: () => Date.now(),
        })
        layerRef.current = layer
        return layer
      },
    })
  }

  return (
    <div ref={boxRef} data-zen-custom-frame-slot={widget.id} className="relative flex min-h-0 min-w-0 flex-1">
      {loaded && (
        <HtmlFrame
          html={loaded.html}
          profile="widget"
          nonce={loaded.nonce}
          prelude={loaded.prelude}
          title={label}
          testId="zen-custom-frame"
          frameRef={(el) => {
            frameRef.current = el
          }}
          onLoad={onLoad}
          className="h-full w-full flex-1"
          style={{ border: 0, background: 'transparent', minHeight: 0 }}
        />
      )}
      {permissionAsk && (
        <ZenPermissionSheet
          ask={permissionAsk.ask}
          onDone={() => {
            permissionAsk.resolve()
            setPermissionAsk(null)
          }}
        />
      )}
      {downloadAsk && (
        <ZenDownloadSheet
          ask={downloadAsk.ask}
          onDone={(answer) => {
            downloadAsk.resolve(answer)
            setDownloadAsk(null)
          }}
        />
      )}
      {linkAsk && (
        <ZenLinkSheet
          ask={linkAsk.ask}
          onDone={(answer) => {
            linkAsk.resolve(answer)
            setLinkAsk(null)
          }}
        />
      )}
      {workAsk && (
        <ZenWorkSheet
          change={workAsk.change}
          widget={label}
          onDone={(yes) => {
            workAsk.resolve(yes)
            setWorkAsk(null)
          }}
        />
      )}
      {guestAsk && (
        <ZenGuestRoomsSheet
          ask={guestAsk.ask}
          onDone={(token) => {
            guestAsk.resolve(token)
            setGuestAsk(null)
          }}
        />
      )}
    </div>
  )
}
