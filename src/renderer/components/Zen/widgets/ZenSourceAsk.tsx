// Garden v3 data sources — K2's "Ask an agent about this" box (PRD §9.4,
// contract-sources-v1 §11.4). Opened from a card's ⋯ menu, a built-in
// card's red / stale / unknown tile, or a widget's `k2.sources.ask` (the
// widget supplies no text). It is the Ask my agent flow (`ZenAskMyAgent`)
// in a K2 overlay, on K2's own bridge: it works whether or not the card has
// `thread:post` (0.46.1 lets a sources card keep it); K2 writes the draft (`zenSourceAskDraft`, no field
// values ever) and the person sends it.
//
// Which agent (Q15): the source's maker when it is an agent on this
// computer, else the one last chosen for this Garden, else the chooser.

import { useCallback } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { scopeForHost } from '@/kessel/server-scope'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { rosterRowsFrom } from '@/lib/zen/zen-custom-scope'
import { parseSourceAddress } from '@/lib/zen/zen-sources'
import { zenSourceScope } from '@/lib/zen/zen-sources-live'
import { showSource } from '@/lib/sources/sources-api'
import {
  rememberZenSourceAskAgent,
  useZenSourceAskStore,
  zenSourceAskDraft,
  zenSourceAskLastAgent,
  zenSourceMakerProject,
  type ZenSourceAskRequest,
} from '@/lib/zen/zen-source-ask'
import { ZenAskMyAgent } from './ZenGardenEmptyWidget'
import { ZenOverlay } from './ZenOverlay'

/** K2's own bridge for the Ask box: list this computer's agents, open one,
 *  fill its message box. Never handed to a widget frame. */
export const ZEN_SOURCE_ASK_ID = 'k2-source-ask'
export const ZEN_SOURCE_ASK_CAPS: readonly string[] = ['agents:read', 'thread:read', 'thread:post']

/** The maker's address on this computer, for a local source with an agent maker. */
export async function zenSourceMakerAddress(req: Pick<ZenSourceAskRequest, 'address' | 'madeBy'>): Promise<string | null> {
  const p = parseSourceAddress(req.address)
  if (!p || p.host) return null
  let madeBy = req.madeBy
  if (!madeBy) {
    const scope = zenSourceScope(null)
    if (!scope) return null
    madeBy = (await showSource(scope, p.name)).source.madeBy
  }
  const project = zenSourceMakerProject(madeBy)
  if (!project) return null
  const rows = rosterRowsFrom(await daemonCliGet<unknown>(scopeForHost('local'), 'projects/list'), 'local')
  return rows.find((r) => r.workspaceId === project)?.address ?? null
}

function AskBox({ bridge, req, onClose }: { bridge: ZenWidgetBridge; req: ZenSourceAskRequest; onClose(): void }): React.JSX.Element {
  const p = parseSourceAddress(req.address)
  const preferred = useCallback(async (): Promise<string | null> => {
    try {
      const maker = await zenSourceMakerAddress(req)
      if (maker) return maker
    } catch (err) {
      console.warn(`[zen] Ask: couldn’t find who made ${req.address}:`, err)
    }
    return zenSourceAskLastAgent(req.gardenId)
  }, [req])
  const decl: ZenWidgetDecl = { id: ZEN_SOURCE_ASK_ID, kind: 'source-ask', column: 0, props: {}, caps: [...ZEN_SOURCE_ASK_CAPS], source: 'builtin' }
  return (
    <ZenOverlay label="Ask an agent" onClose={onClose} testId="zen-source-ask" width={520}>
      <div data-zen-source-ask={req.address} className="flex flex-col" style={{ height: 'min(70vh, 520px)', minHeight: 0 }}>
        <ZenAskMyAgent
          bridge={bridge}
          decl={decl}
          kind="source-ask"
          draft={() => zenSourceAskDraft(req)}
          autoAsk={() => true}
          preferred={preferred}
          onPicked={(address) => rememberZenSourceAskAgent(req.gardenId, address)}
          intro={(ask, phase) => (
            <div className="flex flex-col items-center" style={{ gap: 8 }}>
              <div style={{ fontWeight: 600 }}>Ask an agent about {req.view?.label ?? p?.name ?? req.address}</div>
              <div style={{ color: 'var(--zen-text-muted)', fontSize: '0.9em' }}>
                K2 writes the question; you read it and send it.{p?.host ? ` This check runs on ${p.host}.` : ''}
              </div>
              {phase === 'empty' && (
                <button type="button" data-zen-soft-button="" className="cursor-pointer" onClick={ask} style={{ padding: '4px 12px', borderRadius: 999 }}>
                  Choose an agent
                </button>
              )}
            </div>
          )}
        />
      </div>
    </ZenOverlay>
  )
}

/** Mounted once per Zen page (beside the New Garden host). */
export function ZenSourceAskHost({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element | null {
  const req = useZenSourceAskStore((s) => s.request)
  const close = useZenSourceAskStore((s) => s.close)
  if (!req) return null
  return <AskBox key={`${req.address}\n${req.itemKey ?? ''}`} bridge={bridge} req={req} onClose={close} />
}
