// The chat header's Notify menu (prd-home-chat-pushes-v1 §7, §12): what this
// person's phone gets for the open conversation. Shown only on a server that
// lists `home-chat-push-v1`.
//
//   - a channel: two choices, All messages and @mentions only (no Off). A
//     level of `none` (set in Sew's own client) shows as "Off (set
//     elsewhere)", with both choices still offered;
//   - a DM: the plain line "DMs always notify";
//   - both: the person's push prefs, Show message text and Also notify me
//     for agents' posts.
//
// It is the view's own menu (the context menu the message row's ⋯ opens),
// with the menu's check marks. Picks show at once and go back, with the
// daemon's hint, when it refuses. The levels live in Sew; the daemon reads
// and writes them, and its `notify` frame keeps every device in step
// (`stores/home-chat.ts`).

import { useCallback, useEffect, type JSX } from 'react'
import { BellIcon } from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'
import { daemonErrorText } from '@/lib/daemon-error'
import type { HomeChatNotify, HomeChatPushPrefs } from '@/lib/home-chat/api'
import {
  changeHomeChatPushPrefs,
  pickHomeChatNotify,
  refreshHomeChatNotify,
  refreshHomeChatPushPrefs,
  useHomeChatStore,
} from '@/stores/home-chat'

export const DM_ALWAYS_NOTIFIES = 'DMs always notify'
/** Said when the server owner keeps message text off every phone. */
export const SERVER_TEXT_OFF = 'The server owner turned message text off'
export const NOTIFY_OFF_ELSEWHERE = 'Off (set elsewhere)'
/** Said while the server can't push every message yet (`allMessagesLive`). */
export const ALL_MESSAGES_PENDING = "All messages starts with this server's next chat update; until then you get @mentions."

/** The menu's rows. `notify` is null for a DM (it is fixed) or not read. */
export function homeChatNotifyItems(
  kind: 'dm' | 'channel',
  notify: HomeChatNotify | null,
  prefs: HomeChatPushPrefs | null,
): ContextMenuItemDef[] {
  const items: ContextMenuItemDef[] = [{ id: 'notify-heading', label: 'Notify', type: 'heading' }]
  if (kind === 'dm' || notify?.fixed) {
    items.push({ id: 'notify-dm', label: DM_ALWAYS_NOTIFIES, type: 'note' })
  } else if (notify) {
    if (notify.level === 'none') items.push({ id: 'notify-off', label: NOTIFY_OFF_ELSEWHERE, checked: true, enabled: false })
    items.push({ id: 'level:all', label: 'All messages', checked: notify.level === 'all' })
    items.push({ id: 'level:mentions', label: '@mentions only', checked: notify.level === 'mentions' })
    if (notify.level === 'all' && !notify.allMessagesLive) items.push({ id: 'notify-pending', label: ALL_MESSAGES_PENDING, type: 'note' })
  }
  if (prefs) {
    items.push({ id: 'notify-sep', label: '', type: 'separator' })
    // The owner's switch decides first: off, no text reaches any phone.
    items.push({ id: 'pref:previews', label: 'Show message text', checked: prefs.previews && prefs.serverPreviews, enabled: prefs.serverPreviews })
    if (!prefs.serverPreviews) items.push({ id: 'pref:previews-off', label: SERVER_TEXT_OFF, type: 'note' })
    items.push({ id: 'pref:agents', label: "Also notify me for agents' posts", checked: prefs.agents })
  }
  return items
}

function why(e: unknown): string {
  return (daemonErrorText(e) || String(e)).trim().replace(/[.\s]+$/, '')
}

function warn(text: string, e: unknown): void {
  const reason = why(e)
  useToastStore.getState().addToast(`${text}${reason ? ` (${reason})` : ''}`, 'warning')
}

export function HomeChatNotifyMenu({
  hostKey,
  conv,
  kind,
}: {
  hostKey: string
  /** The conversation (null while a DM is still being opened). */
  conv: string | null
  kind: 'dm' | 'channel'
}): JSX.Element {
  const notify = useHomeChatStore((s) => (conv && kind === 'channel' ? (s.byHost[hostKey]?.notify[conv] ?? null) : null))

  // Read the level (a channel's) and the prefs as the conversation opens,
  // so the menu opens on what the daemon said.
  useEffect(() => {
    if (conv && kind === 'channel') {
      refreshHomeChatNotify(hostKey, conv).catch((e: unknown) => console.warn('[home-chat] notify:', e))
    }
  }, [hostKey, conv, kind])
  useEffect(() => {
    refreshHomeChatPushPrefs(hostKey).catch((e: unknown) => console.warn('[home-chat] push prefs:', e))
  }, [hostKey])

  const pick = useCallback(
    async (id: string | null, n: HomeChatNotify | null, p: HomeChatPushPrefs | null) => {
      if (!id) return
      if ((id === 'level:all' || id === 'level:mentions') && conv && n) {
        const level = id === 'level:all' ? 'all' : 'mentions'
        if (level === n.level) return
        try {
          const got = await pickHomeChatNotify(hostKey, conv, level)
          if (level === 'all' && !got.allMessagesLive) useToastStore.getState().addToast(ALL_MESSAGES_PENDING, 'info')
        } catch (e) {
          warn("Couldn't change this channel's notifications", e)
        }
      } else if ((id === 'pref:previews' || id === 'pref:agents') && p) {
        const patch = id === 'pref:previews' ? { previews: !p.previews } : { agents: !p.agents }
        try {
          await changeHomeChatPushPrefs(hostKey, patch)
        } catch (e) {
          warn("Couldn't change your notification settings", e)
        }
      }
    },
    [hostKey, conv],
  )

  const open = useCallback(
    async (x: number, y: number) => {
      const st = useHomeChatStore.getState().byHost[hostKey]
      let n = conv && kind === 'channel' ? (st?.notify[conv] ?? null) : null
      let p = st?.pushPrefs ?? null
      try {
        if (conv && kind === 'channel' && !n) n = await refreshHomeChatNotify(hostKey, conv)
        if (!p) p = await refreshHomeChatPushPrefs(hostKey)
      } catch (e) {
        warn("Couldn't read your notification settings", e)
        return
      }
      const picked = await useContextMenuStore.getState().show(x, y, homeChatNotifyItems(kind, n, p))
      await pick(picked, n, p)
    },
    [hostKey, conv, kind, pick],
  )

  const label = kind === 'dm' ? DM_ALWAYS_NOTIFIES : notify?.level === 'none' ? NOTIFY_OFF_ELSEWHERE : notify?.level === 'all' ? 'All messages' : notify ? '@mentions only' : 'Notify'
  return (
    <button
      type="button"
      className="k2-focus inline-flex items-center gap-1 rounded-[var(--radius-field)] px-2 py-1 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)]"
      data-testid="home-chat-notify-button"
      aria-haspopup="menu"
      aria-label={`Notify: ${label}`}
      title={`Notify: ${label}`}
      disabled={kind === 'channel' && !conv}
      onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect()
        void open(r.left, r.bottom)
      }}
    >
      <BellIcon size={13} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
      Notify
    </button>
  )
}
