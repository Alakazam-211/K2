// P6 — pure chat-drawer logic (project-chat.ts): load-earlier limit
// stepping, authoritative-tail message merging, PoC labeling, and the
// §6.4 delivered-indicator line mapping.

import { describe, it, expect } from 'vitest'
import {
  CHAT_PANEL_DEFAULT_WIDTH,
  CHAT_PANEL_MAX_WIDTH,
  CHAT_PANEL_MIN_WIDTH,
  EARLIER_PAGE_STEP,
  MESSAGES_DEFAULT_LIMIT,
  MESSAGES_MAX_LIMIT,
  canLoadEarlier,
  clampChatPanelWidth,
  composerPlaceholder,
  deliveredLine,
  mergeMessages,
  nextEarlierLimit,
  pocLabel,
} from './project-chat'
import type { ProjectGroupMemberInfo, ProjectGroupMessage } from './projects-api'

function msg(id: string, createdAt: number, body = id): ProjectGroupMessage {
  return { id, groupId: 'g1', author: 'owner', body, createdAt }
}

describe('nextEarlierLimit / canLoadEarlier (the load-earlier affordance)', () => {
  it('widens the tail window by the step, from the default page', () => {
    expect(nextEarlierLimit(MESSAGES_DEFAULT_LIMIT)).toBe(MESSAGES_DEFAULT_LIMIT + EARLIER_PAGE_STEP)
  })

  it('caps at the daemon 500 ceiling', () => {
    expect(nextEarlierLimit(450)).toBe(MESSAGES_MAX_LIMIT)
    expect(nextEarlierLimit(MESSAGES_MAX_LIMIT)).toBe(MESSAGES_MAX_LIMIT)
  })

  it('offers load-earlier only while truncated AND below the ceiling', () => {
    expect(canLoadEarlier(20, true)).toBe(true)
    expect(canLoadEarlier(20, false)).toBe(false)
    // At the ceiling the drawer shows its "earliest not shown" note instead.
    expect(canLoadEarlier(MESSAGES_MAX_LIMIT, true)).toBe(false)
  })
})

describe('mergeMessages (authoritative-tail merge)', () => {
  it('adopts the fetched page verbatim on first load', () => {
    const page = [msg('a', 100), msg('b', 200)]
    expect(mergeMessages([], page)).toEqual(page)
  })

  it('an identical refetch is a no-op in content', () => {
    const page = [msg('a', 100), msg('b', 200)]
    expect(mergeMessages(page, page)).toEqual(page)
  })

  it('keeps earlier-loaded history as a prefix when a live refetch uses a smaller window', () => {
    // The user "loaded earlier" down to a..d; the live refetch window
    // only covers c..e (a new message arrived). a and b must survive.
    const loaded = [msg('a', 100), msg('b', 200), msg('c', 300), msg('d', 400)]
    const refetch = [msg('c', 300), msg('d', 400), msg('e', 500)]
    expect(mergeMessages(loaded, refetch)).toEqual([
      msg('a', 100),
      msg('b', 200),
      msg('c', 300),
      msg('d', 400),
      msg('e', 500),
    ])
  })

  it('appends a just-posted message (the POST-response echo) at the tail', () => {
    const loaded = [msg('a', 100), msg('b', 200)]
    const posted = msg('p', 250)
    expect(mergeMessages(loaded, [posted])).toEqual([msg('a', 100), msg('b', 200), posted])
  })

  it('dedupes by id when the refetch already contains the posted message', () => {
    const loaded = [msg('a', 100), msg('p', 250)]
    const refetch = [msg('a', 100), msg('p', 250), msg('q', 300)]
    const merged = mergeMessages(loaded, refetch)
    expect(merged.map((m) => m.id)).toEqual(['a', 'p', 'q'])
  })

  it('keeps a same-second boundary row that slid out of the window (older by rowid)', () => {
    // b and c share createdAt 200; the refetch window starts at c —
    // b (lower rowid, outside the window) belongs in the prefix.
    const loaded = [msg('b', 200), msg('c', 200), msg('d', 300)]
    const refetch = [msg('c', 200), msg('d', 300)]
    expect(mergeMessages(loaded, refetch).map((m) => m.id)).toEqual(['b', 'c', 'd'])
  })

  it('drops stale rows newer than the authoritative window head that the window disowns', () => {
    // A phantom row (never confirmed by the daemon) newer than the
    // window head must not survive an authoritative refetch.
    const loaded = [msg('a', 100), msg('ghost', 999)]
    const refetch = [msg('a', 100), msg('b', 200)]
    expect(mergeMessages(loaded, refetch).map((m) => m.id)).toEqual(['a', 'b'])
  })

  it('an anomalous empty page never wipes loaded history', () => {
    const loaded = [msg('a', 100)]
    expect(mergeMessages(loaded, [])).toEqual(loaded)
  })

  it('empty + empty stays empty (a genuinely message-less stream)', () => {
    expect(mergeMessages([], [])).toEqual([])
  })
})

describe('pocLabel', () => {
  const members: ProjectGroupMemberInfo[] = [
    { workspaceId: 'w1', name: 'k2', path: '/k2', agentName: 'Scout', createdAt: 1 },
    { workspaceId: 'w2', name: 'relay', path: '/relay', agentName: null, createdAt: 2 },
  ]

  it('prefers the agent display name, falls back to the workspace name', () => {
    expect(pocLabel(members, 'w1')).toBe('Scout')
    expect(pocLabel(members, 'w2')).toBe('relay')
  })

  it('null when memberless, unknown, or fully unregistered', () => {
    expect(pocLabel(members, null)).toBeNull()
    expect(pocLabel(members, 'w9')).toBeNull()
    expect(
      pocLabel([{ workspaceId: 'w3', name: null, path: null, agentName: null, createdAt: 3 }], 'w3'),
    ).toBeNull()
  })
})

describe('composerPlaceholder (exactly "Message <PoC agent name>")', () => {
  const members: ProjectGroupMemberInfo[] = [
    { workspaceId: 'w1', name: 'k2', path: '/k2', agentName: 'cortana', createdAt: 1 },
    { workspaceId: 'w2', name: 'relay', path: '/relay', agentName: null, createdAt: 2 },
  ]

  it('uses the PoC AGENT name — nothing else in the placeholder', () => {
    expect(composerPlaceholder(members, 'w1')).toBe('Message cortana')
  })

  it('falls back to the workspace name when the agent has no display name', () => {
    expect(composerPlaceholder(members, 'w2')).toBe('Message relay')
  })

  it('"Message the PoC" when no PoC exists or its row is unresolvable', () => {
    expect(composerPlaceholder(members, null)).toBe('Message the PoC')
    expect(composerPlaceholder(members, 'w9')).toBe('Message the PoC')
    expect(
      composerPlaceholder(
        [{ workspaceId: 'w3', name: null, path: null, agentName: null, createdAt: 3 }],
        'w3',
      ),
    ).toBe('Message the PoC')
  })
})

describe('clampChatPanelWidth (§6.7.3 resizable panel clamps)', () => {
  it('passes in-range widths through (rounded to whole px)', () => {
    expect(clampChatPanelWidth(320)).toBe(320)
    expect(clampChatPanelWidth(419.6)).toBe(420)
  })

  it('clamps below the floor and above the ceiling', () => {
    expect(clampChatPanelWidth(0)).toBe(CHAT_PANEL_MIN_WIDTH)
    expect(clampChatPanelWidth(CHAT_PANEL_MIN_WIDTH - 1)).toBe(CHAT_PANEL_MIN_WIDTH)
    expect(clampChatPanelWidth(10_000)).toBe(CHAT_PANEL_MAX_WIDTH)
    expect(clampChatPanelWidth(CHAT_PANEL_MAX_WIDTH + 1)).toBe(CHAT_PANEL_MAX_WIDTH)
  })

  it('non-finite (unset/corrupt localStorage) degrades to the default', () => {
    expect(clampChatPanelWidth(NaN)).toBe(CHAT_PANEL_DEFAULT_WIDTH)
    expect(clampChatPanelWidth(Infinity)).toBe(CHAT_PANEL_DEFAULT_WIDTH)
    expect(clampChatPanelWidth(-Infinity)).toBe(CHAT_PANEL_DEFAULT_WIDTH)
  })
})

describe('deliveredLine (§6.4 indicator mapping)', () => {
  it('delivered → subtle "delivered to <PoC>"', () => {
    expect(deliveredLine(true, null, 'Scout')).toBe('delivered to Scout')
  })

  it('delivered with no resolvable PoC label still reads sanely', () => {
    expect(deliveredLine(true, null, null)).toBe('delivered to the PoC')
  })

  it('author-is-poc renders nothing (should not occur for owner posts — handled gracefully)', () => {
    expect(deliveredLine(false, 'author-is-poc', 'Scout')).toBeNull()
  })

  it('no_poc is a normal stored-only outcome, not an unreachable session', () => {
    expect(deliveredLine(false, 'no_poc', null)).toBe(
      'stored — this project has no Point of Contact yet',
    )
  })

  it('every other failure reason → "PoC session unreachable"', () => {
    expect(deliveredLine(false, 'wake_timeout', 'Scout')).toBe('PoC session unreachable')
    expect(deliveredLine(false, 'no_agent_mode', 'Scout')).toBe('PoC session unreachable')
    expect(deliveredLine(false, 'workspace_not_found', 'Scout')).toBe('PoC session unreachable')
    expect(deliveredLine(false, null, 'Scout')).toBe('PoC session unreachable')
  })
})
