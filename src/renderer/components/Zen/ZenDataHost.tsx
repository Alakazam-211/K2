// prd-zen-mode-v1 S6 (Z42) — the K2 side of `thread.subscribe`.
//
// For every open conversation a Zen widget subscribed to, mount the existing
// overlay Thread hook (`useOverlayThread`: snapshot, live overlay socket,
// paging, answer, void) on the agent's own server and publish what it holds
// to the bridge (`publishZenFeed`). Widgets never see the scope, the Thread
// address or the hook: they get `ZenThreadView`s through the bridge.
//
// prd-zen-gardens-v1 G53: it also runs Home's row-status poll for every
// Home an Agents widget shows, since `HomeShellEffects` only runs on the
// Home page and a Garden's widget may show any Home. For the same reason it
// keeps those Homes' agent pictures current (`useHomeAvatarRowSync`: the
// Home avatar cache Home's rows paint from), so Zen's rows show them too.
// Rendered by `ZenHost` while the window shows Zen; draws nothing.

import { useEffect } from 'react'
import { useStore } from 'zustand'
import { useOverlayThread } from '@/components/SessionView/useOverlayThread'
import { useHomeStatusPoll } from '@/components/Home/home-room'
import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useHomeAvatarRowSync } from '@/lib/home-avatars'
import { clearZenFeed, publishZenFeed, zenThreadFeeds, zenViewHomes, type ZenFeedSpec } from '@/lib/zen/zen-data'

function ZenHomePoll({ home }: { home: Home }): null {
  useHomeStatusPoll(home)
  return null
}

function ZenHomeAvatars({ rows }: { rows: HomeRow[] }): null {
  useHomeAvatarRowSync(rows)
  return null
}

function ZenThreadFeed({ spec }: { spec: ZenFeedSpec }): null {
  const t = useOverlayThread({ scope: spec.scope, addr: spec.threadAddr, conversationId: null, enabled: true })
  const { items, conversationId, loaded, hasMore, loadingOlder, error, loadOlder, answer, voidCard } = t
  useEffect(() => {
    publishZenFeed(
      spec.address,
      { items, conversationId, loaded, hasMore, loadingOlder, error },
      { loadOlder, answer, voidCard },
    )
  }, [spec.address, items, conversationId, loaded, hasMore, loadingOlder, error, loadOlder, answer, voidCard])
  useEffect(() => () => clearZenFeed(spec.address), [spec.address])
  return null
}

export function ZenDataHost(): React.JSX.Element {
  const feeds = useStore(zenThreadFeeds, (s) => s.feeds)
  const homeIds = useStore(zenViewHomes, (s) => s.homeIds)
  const homes = useHomesStore((s) => s.homes)
  const shown = homes.filter((h) => homeIds.includes(h.id))
  // Every row the shown Homes hold, once each, for their pictures.
  const avatarRows: HomeRow[] = []
  const seen = new Set<string>()
  for (const h of shown) {
    for (const r of h.rows) {
      if (seen.has(r.address)) continue
      seen.add(r.address)
      avatarRows.push(r)
    }
  }
  return (
    <>
      {avatarRows.length > 0 && <ZenHomeAvatars rows={avatarRows} />}
      {feeds.map((spec) => (
        <ZenThreadFeed key={`${spec.address}#${spec.generation}`} spec={spec} />
      ))}
      {shown.map((h) => (
        <ZenHomePoll key={h.id} home={h} />
      ))}
    </>
  )
}
