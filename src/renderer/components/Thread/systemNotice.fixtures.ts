// K2's own Thread notices, as the daemon posts them: from `k2`, stamped
// `author: {kind: "system", login: "k2"}` (a stamp only K2 can set), with
// a `systemNotice` that says its kind, its code and, for some, what can be
// done. The words are the daemon's ("K2: " and all), so the tests see what
// a Thread really receives.

import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { CLI_NAME, PRODUCT_NAME } from '@/lib/brand'

const P = PRODUCT_NAME

const AGENT = 'k2-design'

/** K2's stamp on its own lines. */
export const SYSTEM_AUTHOR = { kind: 'system', login: 'k2' } as const

function item(id: string, seq: number, body: string, systemNotice: unknown, via = 'k2'): OverlayThreadItem {
  return {
    collection: 'thread',
    seq,
    id,
    doc: {
      id,
      kind: 'text',
      from: 'k2',
      to: AGENT,
      via,
      body,
      created_at: 1_793_000_000 + seq * 60,
      author: { ...SYSTEM_AUTHOR },
      ...(systemNotice === undefined ? {} : { systemNotice }),
    },
  }
}

const QUOTE = 'the message "Got tokens back."'

/** Send again: K2 sends the original message again (`{thread, doc}`: the
 *  Thread's address and the notice's own id). */
export function resendAction(doc: string, done?: { doneAt: number; doneBy?: unknown }) {
  return { id: 'resend', label: 'Send again', route: '/cli/thread/resend', body: { thread: AGENT, doc }, ...(done ?? {}) }
}
/** The Send again button of the never-read-in notice. */
export const RESEND_ACTION = resendAction('n-never')
/** Open its terminal: the client opens the named agent's terminal. */
export const OPEN_TERMINAL_ACTION = { id: 'open_terminal', label: 'Open its terminal', workspace: AGENT }

const delivery = (id: string, code: string) => ({ kind: 'delivery', code, actions: [resendAction(id), OPEN_TERMINAL_ACTION] })

/** The four ways a Thread message can fail to reach its agent. */
export const DELIVERY_NOTICES = {
  neverReadIn: item(
    'n-never',
    1,
    `${P}: ${AGENT} didn't take ${QUOTE}. ${P} typed it into ${AGENT}'s terminal twice, but the agent never read it in. Its screen may be on another view. Check its terminal, then send the message again.`,
    delivery('n-never', 'delivery.not_taken.never_read_in'),
    'thread',
  ),
  retryFailed: item(
    'n-retry',
    2,
    `${P}: ${AGENT} didn't take ${QUOTE}, and ${P} couldn't type it in again. Check its terminal, then send the message again.`,
    delivery('n-retry', 'delivery.not_taken.retry_failed'),
    'thread',
  ),
  restartMidDelivery: item(
    'n-restart',
    3,
    `${P}: ${P} itself restarted while it was typing ${QUOTE} into ${AGENT}'s terminal, so it may not have reached ${AGENT}. Check before you send it again.`,
    delivery('n-restart', 'delivery.not_taken.restart_mid_delivery'),
    'thread',
  ),
  restartTooOld: item(
    'n-old',
    4,
    `${P}: ${QUOTE} was still waiting for ${AGENT} when ${P} restarted, and it was over an hour old, so ${P} didn't send it. Send it again if it still matters.`,
    delivery('n-old', 'delivery.not_taken.restart_too_old'),
    'thread',
  ),
} as const

/** Jobs added, changed and removed; an App's test guest; a landed worktree. */
export const INFO_NOTICES = {
  jobAdded: item(
    'n-job-add',
    5,
    `${P}: job **nightly-backup** added by ada: every day at 02:00, runs \`scripts/backup.sh\` (timeout 30m). It runs unattended with only the secrets granted to job:nightly-backup. \`${CLI_NAME} job disable nightly-backup\` stops it.`,
    { kind: 'info', code: 'job.added' },
    'k2-job',
  ),
  jobChanged: item(
    'n-job-edit',
    6,
    `${P}: job **nightly-backup** changed by ada: every day at 03:00, runs \`scripts/backup.sh\` (timeout 30m). It runs unattended with only the secrets granted to job:nightly-backup. \`${CLI_NAME} job disable nightly-backup\` stops it.`,
    { kind: 'info', code: 'job.changed' },
    'k2-job',
  ),
  jobRemoved: item('n-job-rm', 7, `${P}: job **nightly-backup** removed by ada.`, { kind: 'info', code: 'job.removed' }, 'k2-job'),
  testGuest: item(
    'n-guest',
    8,
    `${P}: ada added the test guest **qa-1** to App **orders** (role tester, for 2 hours). It signs in only through the App's gateway link and ends on its own. \`${CLI_NAME} app user remove qa-1 --test\` removes it now.`,
    { kind: 'info', code: 'app.test_guest_added' },
    'k2-apps',
  ),
  /** A sidecar's worktree line: the "[K2] " lead. */
  landed: item('n-landed', 9, `[${P}] Landed 2 commits from design/notices on main (a1b2c3d..d4e5f6a) for grader-a11y.`, {
    kind: 'info',
    code: 'sidecar.landed',
  }),
} as const

/** A notice that asks for a step, with a button. */
export const ACTION_NOTICE = item('n-action', 10, `${P}: The agent asked to resend the last message.`, {
  kind: 'action',
  code: 'resend.offer',
  actions: [resendAction('n-action')],
})

/** An agent's own post that starts with "K2: " and carries a look-alike
 *  `systemNotice`: no system stamp, so it stays the agent's bubble. */
export const LOOKALIKE_AGENT_POST: OverlayThreadItem = {
  collection: 'thread',
  seq: 11,
  id: 'n-lookalike',
  doc: {
    id: 'n-lookalike',
    kind: 'text',
    from: AGENT,
    author: { kind: 'agent', login: AGENT },
    body: `${P}: ${AGENT} didn't take ${QUOTE}. Check its terminal, then send the message again.`,
    created_at: 1_793_000_660,
    systemNotice: { kind: 'delivery', code: 'delivery.not_taken.never_read_in', actions: [RESEND_ACTION] },
  },
}

/** An old daemon's notice: "K2: " in the words, no stamp and no notice. */
export const OLD_DAEMON_NOTICE: OverlayThreadItem = {
  collection: 'thread',
  seq: 12,
  id: 'n-old-daemon',
  doc: {
    id: 'n-old-daemon',
    kind: 'text',
    from: AGENT,
    body: `${P}: ${AGENT} didn't take ${QUOTE}, and ${P} couldn't type it in again. Check its terminal, then send the message again.`,
    created_at: 1_793_000_720,
  },
}

/** K2's mail notice: stamped and with a `systemNotice` too, but its
 *  `notice` is the mail card, which keeps drawing as that card. */
export const MAIL_NOTICE_CARD: OverlayThreadItem = {
  collection: 'thread',
  seq: 13,
  id: 'n-mail',
  doc: {
    id: 'n-mail',
    kind: 'text',
    from: 'k2',
    via: 'k2',
    author: { ...SYSTEM_AUTHOR },
    body: `${P}: new mail from ada@example.com`,
    created_at: 1_793_000_780,
    notice: { source: 'imap', inbox: 'orders', fromAddr: 'ada@example.com', auth: 'pass', subject: 'Invoice', readCommand: `${CLI_NAME} mail read 7`, delivered: true, noticeId: 7 },
    systemNotice: { kind: 'info', code: 'mail.vip' },
  },
}
