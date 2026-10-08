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
//    checking every message against the widget's caps, scope and budgets.
// A problem loading (an older daemon, no good bundle, a library that can't
// be served) goes to the parent, which draws K2's card instead. The frame
// is remounted (keyed by the parent) on a new hash or a Reload.

import { useEffect, useMemo, useRef, useState } from 'react'
import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameHello } from '@/lib/zen/zen-custom-types'
import { fetchZenWidgetBundle, setZenWidgetSending, zenWidgetError, type ZenWidgetErrorCode } from '@/lib/zen/zen-custom-grants'
import { zenLib, ZenLibError } from '@/lib/zen/zen-lib-loader'
import { zenFramePrelude, type ZenFrameTheme } from '@/lib/zen/zen-custom-prelude'
import { createZenCustomLayer } from '@/lib/zen/zen-custom-bridge'
import { startZenFrameHost } from '@/lib/zen/zen-custom-host'
import { stopZenWidget, zenPlacementKey } from '@/lib/zen/zen-custom-run'
import { ensureZenConversation, zenAgentRows, zenCustomViewFor } from '@/lib/zen/zen-data'
import { zenWidgetDisplayName } from '@/lib/zen/zen-custom-words'

/** Why the bundle couldn't be shown (the parent draws K2's card). */
export interface ZenCustomLoadProblem {
  code: ZenWidgetErrorCode
  message: string
}

/** The `/boot-status` features the widget rows need: this daemon served the
 *  bundle, so it has them (UW36). B1's generated table names each row's. */
export const ZEN_FRAME_FEATURES: readonly string[] = ['zen-v1', 'zen-gardens-v1', 'zen-widgets-v1']

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

type Loaded = { html: string; nonce: string; prelude: string }

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
  const boxRef = useRef<HTMLDivElement | null>(null)
  const frameRef = useRef<HTMLIFrameElement | null>(null)
  const hostRef = useRef<{ dispose(): void } | null>(null)
  const widgetRef = useRef(widget)
  widgetRef.current = widget
  const problemRef = useRef(onProblem)
  problemRef.current = onProblem
  const key = zenPlacementKey(gardenId, widget.id)

  // 1. Bundle and libraries (once per mount: the parent keys us by hash).
  useEffect(() => {
    let live = true
    const root = boxRef.current?.closest<HTMLElement>('[data-zen-root]') ?? null
    void Promise.all([fetchZenWidgetBundle(widget.widget), zenLib.texts(widget.libs)])
      .then(([bundle, libs]) => {
        if (!live) return
        const prelude = zenFramePrelude({ nonce: bundle.nonce, theme: readZenFrameTheme(root), libs })
        setLoaded({ html: bundle.html, nonce: bundle.nonce, prelude })
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
      focused: () => document.activeElement === frameRef.current,
      makeLayer: (push) =>
        createZenCustomLayer({
          widget: () => widgetRef.current,
          gardenId: () => gardenId,
          inner: bridge,
          boundRows: () => zenAgentRows(zenCustomViewFor(gardenId, widgetRef.current)),
          ensureConversation: (address) => ensureZenConversation(zenCustomViewFor(gardenId, widgetRef.current), address),
          push,
          stop: (reason) => stopZenWidget(key, reason),
          sendingOffForRunaway: () =>
            setZenWidgetSending({ garden: gardenId, placement: widgetRef.current.id, on: false, reason: 'runaway' }),
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
          now: () => Date.now(),
        }),
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
    </div>
  )
}
