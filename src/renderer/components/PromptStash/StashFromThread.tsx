// Saving into the prompt stash from a Thread (spec-prompt-stash-v1):
//   - "Save to stash" in a message's menu (right-click, a long press, or
//     the ⋯ beside "author · time"; the menu is
//     components/Thread/ThreadMessageMenu, prd-thread-message-menu-v1);
//   - the `stash.offer` card: an agent offers a prompt, the person's tap
//     saves it on this device. Agents never write the stash themselves,
//     and the tap sends nothing back to the daemon.

import { useRef, useState, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayStashOffer } from '@/components/SessionView/overlayThread'
import { stashSourceFor } from '@/lib/prompt-stash/compose'
import { orNull, stashOfferView, type StashSource } from '@/lib/prompt-stash/record'
import { saveToStash } from '@/stores/prompt-stash'
import { useToastStore } from '@/stores/toast'
import { StashGlyph } from './PromptStash'

function sourceFor(scope: ServerScope | null, agent: string | null, agentName: string | null): Promise<StashSource> {
  if (scope) return stashSourceFor(scope, agent, agentName)
  return Promise.resolve({ server_id: null, server_label: null, user: null, agent: orNull(agent), agent_name: orNull(agentName) })
}

/** Save a Thread message's text to the stash; true when it was saved. */
export async function stashThreadText(args: {
  scope: ServerScope | null
  text: string
  /** The sender's address (the message's `from`, group chats too); on the
   *  person's own message, the Thread it was written to. */
  agent: string | null
  agentName: string | null
  /** A reply's quoted words (prd-thread-message-menu-v1), kept as quotes. */
  quotes?: Array<{ text: string; from?: string | null }>
}): Promise<boolean> {
  const text = args.text.trim()
  const quotes = (args.quotes ?? []).filter((q) => q.text.trim().length > 0)
  if (!text && quotes.length === 0) return false
  try {
    const r = await saveToStash({
      text,
      ...(quotes.length > 0 ? { quotes } : {}),
      source: await sourceFor(args.scope, args.agent, args.agentName),
    })
    if (r.status !== 'saved') return false
    useToastStore.getState().addToast('Saved to your stash', 'success', 2500)
    return true
  } catch (err) {
    useToastStore.getState().addToast(`Couldn't save to your stash: ${err instanceof Error ? err.message : String(err)}`, 'error')
    return false
  }
}

/** Offers saved in this window, by Thread message id: one tap is one
 *  save, and the card still says "Saved" when it is drawn again. */
const savedOffers = new Set<string>()

/** Tests: forget which offers were saved. */
export function resetSavedOffersForTests(): void {
  savedOffers.clear()
}

/** The `stash.offer` card inside an agent's bubble (field rules: title
 *  ≤ 200 and text ≤ 20,000 characters, cut with a note; a tap saves
 *  exactly what the card shows, title, blank line, text). */
export function StashOfferCard({
  docId,
  offer,
  scope,
  agent,
  agentName,
}: {
  /** The Thread message's id. */
  docId: string
  offer: OverlayStashOffer
  scope: ServerScope | null
  agent: string | null
  agentName: string | null
}): JSX.Element {
  const view = stashOfferView(offer)
  const [state, setState] = useState<'idle' | 'saving' | 'saved'>(() => (savedOffers.has(docId) ? 'saved' : 'idle'))
  // A second tap before the first re-renders must not save again.
  const busy = useRef(false)
  const save = async (): Promise<void> => {
    if (busy.current || savedOffers.has(docId)) return
    busy.current = true
    setState('saving')
    const ok = await stashThreadText({ scope, text: view.saved, agent, agentName })
    if (ok) savedOffers.add(docId)
    else busy.current = false
    setState(ok ? 'saved' : 'idle')
  }
  return (
    <div
      data-testid="stash-offer-card"
      className="mt-1 box-border flex w-full flex-col gap-1.5 border px-2 py-1.5"
      style={{
        borderColor: 'var(--thread-card-border, var(--color-border))',
        background: 'var(--thread-card-bg, var(--color-bg))',
        borderRadius: 'var(--thread-card-radius, 0px)',
        color: 'var(--thread-card-text, var(--color-text-primary))',
      }}
    >
      {view.title ? (
        <div data-stash-offer-title="" style={{ fontWeight: 600, fontSize: '0.92em', overflowWrap: 'anywhere' }}>
          {view.title}
        </div>
      ) : null}
      <div
        data-stash-offer-text=""
        className="selectable-copy"
        style={{ whiteSpace: 'pre-wrap', fontSize: '0.88em', maxHeight: '9em', overflowY: 'auto', overflowWrap: 'anywhere' }}
      >
        {view.text}
      </div>
      {view.truncated ? (
        <div data-stash-offer-truncated="" style={{ fontSize: '0.78em', color: 'var(--thread-card-muted, var(--color-text-muted))' }}>
          This offer was too long and was shortened. Saving keeps what you see here.
        </div>
      ) : null}
      <button
        type="button"
        data-testid="stash-offer-save"
        disabled={state !== 'idle'}
        onClick={() => void save()}
        className="inline-flex cursor-pointer items-center justify-center gap-1.5 self-start border px-2 py-1 text-[11px] font-medium disabled:cursor-default"
        style={{
          borderColor: 'var(--thread-card-accent, var(--color-accent))',
          borderRadius: 'var(--thread-card-radius, 0px)',
          color: 'var(--thread-card-text, var(--color-text-primary))',
          background: 'none',
          opacity: state === 'saving' ? 0.6 : 1,
        }}
      >
        <StashGlyph size={11} />
        {state === 'saved' ? 'Saved' : 'Save to stash'}
      </button>
    </div>
  )
}
