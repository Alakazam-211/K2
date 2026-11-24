// Plain words for the server-settings rows in Settings → Logs → Access
// Audit: Send feedback (`product-feedback-settings`), the ticket rules
// (`tickets-settings`) and the Org Chart switch (`org_chart.switch`). The
// outcomes are the daemon's own strings (product_feedback_routes.rs
// `audit_changes`, ticket_settings_routes.rs `handle_post`, settings_routes.rs
// `sync_org_chart`). An outcome these words don't know reads as before.

import { describe, it, expect } from 'vitest'
import type { AuthAuditEvent } from './access-audit-api'
import { auditDetail, auditFields, auditWhat, auditWho, auditWhoText, type DetailContext } from './access-audit-words'
import { PRODUCT_NAME } from '@/lib/brand'

function ev(event: string, outcome: string, user = 'owner'): AuthAuditEvent {
  return { ts: '2026-11-18T20:00:00Z', event, user, outcome, ingress: '-', ip: '-', client: 'cli' }
}

const ctx: DetailContext = { ws: (k) => k, requests: new Map() }
const when = { local: 'Nov 18, 1:00 PM', utc: '2026-11-18 20:00:00' }

function field(e: AuthAuditEvent, label: string): string | undefined {
  return auditFields(e, ctx, when).find((f) => f.label === label)?.value
}

describe('product-feedback-settings', () => {
  it.each([
    ['ok enabled off->on', `turned feedback to ${PRODUCT_NAME} on`],
    ['ok enabled on->off', `turned feedback to ${PRODUCT_NAME} off`],
    ['ok agents off->on', `turned agents’ feedback to ${PRODUCT_NAME} on`],
    ['ok agents on->off', `turned agents’ feedback to ${PRODUCT_NAME} off`],
    ['ok agentDiagnostics off->on', `let agents include ${PRODUCT_NAME} details in their feedback`],
    ['ok agentDiagnostics on->off', `stopped agents including ${PRODUCT_NAME} details in their feedback`],
  ])('%s reads as a sentence', (outcome, want) => {
    expect(auditWhat(ev('product-feedback-settings', outcome))).toBe(want)
    expect(auditDetail(ev('product-feedback-settings', outcome), ctx)).toBe('')
  })

  it.each(['ok rememberDiagnostics off->on', 'ok agents on->on', 'ok agents maybe->on', 'ok', 'refused', ''])(
    'an outcome it does not know (%j) still reads "did something"',
    (outcome) => {
      expect(auditWhat(ev('product-feedback-settings', outcome))).toBe('did something')
      expect(auditDetail(ev('product-feedback-settings', outcome), ctx)).toBe('')
    },
  )

  it('the opened record names the event and keeps the raw line', () => {
    const e = ev('product-feedback-settings', 'ok agents on->off', 'login:alex')
    expect(field(e, 'What')).toBe('Send feedback settings')
    expect(auditFields(e, ctx, when).find((f) => f.label === 'Log line')?.raw).toBe('ok agents on->off')
  })
})

describe('tickets-settings', () => {
  it.each([
    ['ok briefPolicy=off', 'stopped asking agents for an HTML brief on tickets'],
    ['ok briefPolicy=warn', 'set tickets to warn an agent that leaves out the HTML brief'],
    ['ok briefPolicy=require', 'made agents attach an HTML brief to every ticket'],
    ['ok assigneePolicy=off', 'stopped asking agents to name an assignee on tickets'],
    ['ok assigneePolicy=warn', 'set tickets to warn an agent that names no assignee'],
    ['ok assigneePolicy=require', 'made agents name an assignee on every ticket'],
    ['ok defaultAssignee=owner', 'set the default ticket assignee to the owner'],
    ['ok defaultAssignee=alex', 'set the default ticket assignee to alex'],
    ['ok defaultAssignee=', 'cleared the default ticket assignee'],
  ])('one change, %s, reads as a sentence with no second line', (outcome, want) => {
    expect(auditWhat(ev('tickets-settings', outcome))).toBe(want)
    expect(auditDetail(ev('tickets-settings', outcome), ctx)).toBe('')
  })

  it('several changes read "changed the ticket rules" and list each in the second line', () => {
    const e = ev('tickets-settings', 'ok assigneePolicy=require briefPolicy=warn defaultAssignee=', 'login:alex')
    expect(auditWhat(e)).toBe('changed the ticket rules')
    expect(auditDetail(e, ctx)).toBe('Require an assignee: Require · Require an HTML brief: Warn · Default assignee: nobody')
    expect(field(e, 'Details')).toBe('Require an assignee: Require · Require an HTML brief: Warn · Default assignee: nobody')
    expect(field(e, 'What')).toBe('Ticket rules')
  })

  it.each([
    ['refused:agent', 'An agent can’t change the ticket rules'],
    ['refused:member', 'Only the owner or an admin may change the ticket rules'],
    ['refused:invalid', 'No valid sign-in'],
    ['refused:bad_request', 'The change wasn’t understood'],
    ['refused:unknown_user', 'The default assignee isn’t a person on this server'],
    ['refused:update', 'The change couldn’t be saved'],
  ])('%s is a refusal with its reason', (outcome, why) => {
    const e = ev('tickets-settings', outcome, 'login:sam')
    expect(auditWhat(e)).toBe('was refused a change to the ticket rules')
    expect(auditDetail(e, ctx)).toBe(why)
  })

  it('a refusal code it does not know adds no reason', () => {
    const e = ev('tickets-settings', 'refused:something_new')
    expect(auditWhat(e)).toBe('was refused a change to the ticket rules')
    expect(auditDetail(e, ctx)).toBe('')
  })

  it.each(['ok', 'ok briefPolicy=sometimes', 'ok newRule=on', 'ok briefPolicy=warn newRule=on', 'error: disk', ''])(
    'an outcome it does not know (%j) still reads "did something"',
    (outcome) => {
      expect(auditWhat(ev('tickets-settings', outcome))).toBe('did something')
      expect(auditDetail(ev('tickets-settings', outcome), ctx)).toBe('')
    },
  )
})

describe('org_chart.switch', () => {
  it.each([
    ['on', 'turned the Org Chart on'],
    ['off', 'turned the Org Chart off'],
  ])('%s reads as a sentence', (outcome, want) => {
    const e = ev('org_chart.switch', outcome)
    expect(auditWhat(e)).toBe(want)
    expect(auditDetail(e, ctx)).toBe('')
    expect(field(e, 'What')).toBe('Org Chart switch')
  })

  it.each(['ok on', 'paused', ''])('an outcome it does not know (%j) still reads "did something"', (outcome) => {
    expect(auditWhat(ev('org_chart.switch', outcome))).toBe('did something')
  })
})

describe('other events read as before', () => {
  it.each([
    ['login', 'ok', 'signed in'],
    ['login', 'web_client_disabled', 'was blocked: web access to this server is off'],
    ['logout', 'ok', 'signed out'],
    ['vault-handoff', 'ok send GITHUB_TOKEN to acme 7 pending', 'sent a secret'],
    ['vault-carry', 'ok import 2 from v1 into acme', 'imported secrets into this server’s vault'],
    ['permission-switch', 'server on', 'changed a permission switch'],
    ['some-new-event', 'ok enabled off->on', 'did something'],
  ])('%s %j → %s', (event, outcome, want) => {
    expect(auditWhat(ev(event, outcome))).toBe(want)
  })

  it('an event it does not know is still "Other" in the opened record', () => {
    expect(field(ev('some-new-event', 'on'), 'What')).toBe('Other')
  })
})

describe('a refused agent passport', () => {
  it('reads "An agent", not the raw name, and never "An agent agent"', () => {
    expect(auditWho('agent-passport')).toEqual({ name: 'An agent', agent: false })
    expect(auditWhoText('agent-passport')).toBe('An agent')
  })
  it('leaves a named agent as it was', () => {
    expect(auditWho('agent:sales/s1', (w) => w)).toEqual({ name: 'sales', agent: true, session: 's1' })
  })
})
