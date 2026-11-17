// K2's own Thread notices, as the daemon posts them: under the agent's
// address, stamped `author: {kind: "system"}` (a stamp only K2 can set),
// with a `notice` that says its kind and, for some, what can be done.
// The words are the daemon's (`K2: ` and all), so the tests see what a
// Thread really receives.

import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

const AGENT = 'k2-design'

function item(id: string, seq: number, body: string, notice: unknown, from = AGENT): OverlayThreadItem {
  return {
    collection: 'thread',
    seq,
    id,
    doc: {
      id,
      kind: 'text',
      from,
      body,
      created_at: 1_793_000_000 + seq * 60,
      author: { kind: 'system' },
      ...(notice === undefined ? {} : { notice }),
    },
  }
}

const QUOTE = 'the message "Got tokens back."'

/** The Send again button: K2 types the message in again. */
export const RESEND_ACTION = {
  id: 'resend',
  label: 'Send again',
  route: '/cli/threads/resend',
  body: { thread: AGENT, doc: 'd-tokens' },
}
/** Open its terminal: the client opens the agent's terminal view. */
export const OPEN_TERMINAL_ACTION = { id: 'open_terminal', label: 'Open its terminal' }

/** The four ways a Thread message can fail to reach its agent. */
export const DELIVERY_NOTICES = {
  neverReadIn: item(
    'n-never',
    1,
    `K2: ${AGENT} didn't take ${QUOTE}. K2 typed it into ${AGENT}'s terminal twice, but the agent never read it in. Its screen may be on another view. Check its terminal, then send the message again.`,
    { kind: 'delivery', code: 'not_taken.never_read_in', actions: [RESEND_ACTION, OPEN_TERMINAL_ACTION] },
  ),
  retryFailed: item(
    'n-retry',
    2,
    `K2: ${AGENT} didn't take ${QUOTE}, and K2 couldn't type it in again. Check its terminal, then send the message again.`,
    { kind: 'delivery', code: 'not_taken.retry_failed', actions: [RESEND_ACTION, OPEN_TERMINAL_ACTION] },
  ),
  restartMidDelivery: item(
    'n-restart',
    3,
    `K2: K2 itself restarted while it was typing ${QUOTE} into ${AGENT}'s terminal, so it may not have reached ${AGENT}. Check before you send it again.`,
    { kind: 'delivery', code: 'not_taken.restart_mid_delivery', actions: [OPEN_TERMINAL_ACTION, RESEND_ACTION] },
  ),
  restartTooOld: item(
    'n-old',
    4,
    `K2: ${QUOTE} was still waiting for ${AGENT} when K2 restarted, and it was over an hour old, so K2 didn't send it. Send it again if it still matters.`,
    { kind: 'delivery', code: 'not_taken.restart_too_old', actions: [RESEND_ACTION] },
  ),
} as const

/** Jobs added, changed and removed; an App's test guest; a file saved. */
export const INFO_NOTICES = {
  jobAdded: item(
    'n-job-add',
    5,
    'K2: job **nightly-backup** added by ada: every day at 02:00, runs `scripts/backup.sh` (timeout 30m). It runs unattended with only the secrets granted to job:nightly-backup. `k2 job disable nightly-backup` stops it.',
    { kind: 'info', code: 'job.added', actions: [] },
    'k2-job',
  ),
  jobChanged: item(
    'n-job-edit',
    6,
    'K2: job **nightly-backup** changed by ada: every day at 03:00, runs `scripts/backup.sh` (timeout 30m). It runs unattended with only the secrets granted to job:nightly-backup. `k2 job disable nightly-backup` stops it.',
    { kind: 'info', code: 'job.changed' },
    'k2-job',
  ),
  jobRemoved: item('n-job-rm', 7, 'K2: job **nightly-backup** removed by ada.', { kind: 'info', code: 'job.removed' }, 'k2-job'),
  testGuest: item(
    'n-guest',
    8,
    'K2: ada added the test guest **qa-1** to App **orders** (role tester, for 2 hours). It signs in only through the App\'s gateway link and ends on its own. `k2 app user remove qa-1 --test` removes it now.',
    { kind: 'info', code: 'app.test_guest_added' },
    'k2-apps',
  ),
  fileSaved: item('n-saved', 9, 'K2: saved agent-orders/out/report.txt', { kind: 'info', code: 'file.saved' }),
} as const

/** A notice that asks for a step, with a button. */
export const ACTION_NOTICE = item(
  'n-action',
  10,
  'K2: The agent asked to resend the last message.',
  { kind: 'action', code: 'resend.offer', actions: [RESEND_ACTION] },
)

/** An agent's own post that starts with "K2: " (an old daemon's notice, or
 *  an agent quoting one): no system stamp, so it stays the agent's bubble. */
export const LOOKALIKE_AGENT_POST: OverlayThreadItem = {
  collection: 'thread',
  seq: 11,
  id: 'n-lookalike',
  doc: {
    id: 'n-lookalike',
    kind: 'text',
    from: AGENT,
    body: `K2: ${AGENT} didn't take ${QUOTE}. Check its terminal, then send the message again.`,
    created_at: 1_793_000_660,
    notice: { kind: 'delivery', code: 'not_taken.never_read_in', actions: [RESEND_ACTION] },
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
    body: `K2: ${AGENT} didn't take ${QUOTE}, and K2 couldn't type it in again. Check its terminal, then send the message again.`,
    created_at: 1_793_000_720,
  },
}
