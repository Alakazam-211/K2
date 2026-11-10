// Email Link: every daemon reply the page reads goes through a shape guard.
// A missing optional field becomes its default; a reply that isn't the
// expected shape throws MailReplyError (the page shows a failed read with
// Try again) instead of crashing the page or reading as an empty list.
import { describe, expect, it } from 'vitest'
import {
  MailReplyError,
  mailErrorInfo,
  parseInbox,
  parseInboxes,
  parseOauthConfig,
  parseOauthStart,
  parseOauthStatus,
} from './email-api'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const GOOD_ROW = {
  address: 'alex@example.com',
  source: 'linked',
  displayName: 'Alex',
  status: 'connected',
  primary: { projectId: 'p1', workspace: 'atlas', level: 'draft', canManage: true, canDelete: false, agentSend: 'on', sendState: 'no_send_level' },
  grants: [{ projectId: 'p2', workspace: 'release-bot', level: 'read', canManage: false, canDelete: false }],
  yourLevel: null,
  maxLevel: 'send',
  host: 'imap.gmail.com',
  tls: 'implicit-tls',
  capabilities: { source: 'google', mail: { read: true }, calendar: { read: false }, missing: [{ cap: 'calendar.read', reason: 'scope_not_granted', fix: 'upgrade' }] },
}

describe('parseInboxes', () => {
  it('reads a good reply', () => {
    const rows = parseInboxes({ ok: true, count: 1, inboxes: [GOOD_ROW] })
    expect(rows).toHaveLength(1)
    expect(rows[0]).toMatchObject({
      address: 'alex@example.com',
      source: 'linked',
      status: 'connected',
      maxLevel: 'send',
      primary: { projectId: 'p1', workspace: 'atlas', level: 'draft', canManage: true, agentSend: 'on' },
      grants: [{ projectId: 'p2', level: 'read' }],
    })
    expect(rows[0].capabilities?.missing).toEqual([{ cap: 'calendar.read', reason: 'scope_not_granted', fix: 'upgrade' }])
  })

  it('reads an empty list as no accounts', () => {
    expect(parseInboxes({ ok: true, count: 0, inboxes: [] })).toEqual([])
  })

  it('refuses {} rather than showing an empty list', () => {
    expect(() => parseInboxes({})).toThrow(MailReplyError)
  })

  it('refuses a bare list', () => {
    expect(() => parseInboxes([])).toThrow(MailReplyError)
  })

  it('refuses a wrong type', () => {
    expect(() => parseInboxes({ inboxes: 'alex@example.com' })).toThrow(MailReplyError)
    expect(() => parseInboxes('ok')).toThrow(MailReplyError)
    expect(() => parseInboxes(null)).toThrow(MailReplyError)
  })

  it('names what it was reading', () => {
    try {
      parseInboxes({})
      throw new Error('expected a throw')
    } catch (e) {
      expect(e).toBeInstanceOf(MailReplyError)
      expect((e as MailReplyError).what).toBe('your email accounts')
    }
  })
})

describe('parseInbox', () => {
  it('fills missing optional fields with defaults', () => {
    const row = parseInbox({ address: 'a@example.com', source: 'linked', primary: { projectId: 'p1' } })
    expect(row).toEqual({
      address: 'a@example.com',
      source: 'linked',
      displayName: null,
      status: 'unknown',
      primary: { projectId: 'p1', workspace: null, level: 'read', canManage: false, canDelete: false, agentSend: undefined, sendState: undefined },
      grants: [],
      yourLevel: null,
      yourAgentSend: null,
      yourSendState: null,
      maxLevel: 'draft',
      domain: null,
      host: null,
      tls: null,
      capabilities: undefined,
    })
  })

  it('never carries delete without management', () => {
    const row = parseInbox({ ...GOOD_ROW, primary: { projectId: 'p1', canManage: false, canDelete: true } })
    expect(row.primary.canDelete).toBe(false)
  })

  it('drops an unknown level to read and an unknown capability source to none', () => {
    const row = parseInbox({ ...GOOD_ROW, primary: { projectId: 'p1', level: 'admin' }, capabilities: { source: 'pop3' } })
    expect(row.primary.level).toBe('read')
    expect(row.capabilities).toBeUndefined()
  })

  it('refuses a row without an address, a source or a Primary', () => {
    expect(() => parseInbox({})).toThrow(MailReplyError)
    expect(() => parseInbox([])).toThrow(MailReplyError)
    expect(() => parseInbox({ ...GOOD_ROW, source: 'pop3' })).toThrow(MailReplyError)
    expect(() => parseInbox({ ...GOOD_ROW, primary: undefined })).toThrow(MailReplyError)
    expect(() => parseInbox({ ...GOOD_ROW, grants: [{}] })).toThrow(MailReplyError)
  })
})

describe('parseOauthConfig', () => {
  it('reads a good reply', () => {
    const cfg = parseOauthConfig({
      ok: true,
      providers: {
        gmail: { source: 'custom', clientId: 'id.apps.example', secretSet: true, configured: true },
        microsoft: { source: 'default', clientId: 'ms-id', secretSet: false, configured: false, tenant: 'common' },
      },
    })
    expect(cfg.providers.gmail).toEqual({ source: 'custom', clientId: 'id.apps.example', secretSet: true, configured: true, tenant: undefined })
    expect(cfg.providers.microsoft.configured).toBe(false)
    expect(cfg.providers.microsoft.tenant).toBe('common')
  })

  it('defaults a provider the reply leaves out (an older server)', () => {
    const cfg = parseOauthConfig({ providers: {} })
    expect(cfg.providers.gmail).toEqual({ source: 'default', secretSet: false })
    expect(cfg.providers.microsoft.configured).toBeUndefined()
  })

  it('refuses {}, a list and a wrong type (the crash under an empty reply)', () => {
    expect(() => parseOauthConfig({})).toThrow(MailReplyError)
    expect(() => parseOauthConfig([])).toThrow(MailReplyError)
    expect(() => parseOauthConfig({ providers: 'gmail' })).toThrow(MailReplyError)
  })
})

describe('parseOauthStart', () => {
  it('reads a device flow and a loopback flow', () => {
    expect(parseOauthStart({ flow: 'device', linkId: 'l1', userCode: 'FAKE-0001', verificationUrl: 'https://login.example/device', expiresIn: 900 })).toEqual({
      flow: 'device', linkId: 'l1', userCode: 'FAKE-0001', verificationUrl: 'https://login.example/device', expiresIn: 900,
    })
    expect(parseOauthStart({ flow: 'loopback', linkId: 'l2', clientCapture: true, authorizationUrl: 'https://accounts.example/auth', state: 's' })).toMatchObject({
      flow: 'loopback', linkId: 'l2', clientCapture: true, authorizationUrl: 'https://accounts.example/auth', state: 's',
    })
  })

  it('defaults a missing expiry to none', () => {
    expect(parseOauthStart({ flow: 'device', linkId: 'l1', userCode: 'C', verificationUrl: 'https://x.example' })).toMatchObject({ expiresIn: 0 })
  })

  it('refuses {}, a list, a wrong type and a device flow without its code', () => {
    expect(() => parseOauthStart({})).toThrow(MailReplyError)
    expect(() => parseOauthStart([])).toThrow(MailReplyError)
    expect(() => parseOauthStart({ flow: 'pigeon', linkId: 'l1' })).toThrow(MailReplyError)
    expect(() => parseOauthStart({ flow: 'device', linkId: 'l1' })).toThrow(MailReplyError)
  })
})

describe('parseOauthStatus', () => {
  it('reads a good reply', () => {
    expect(parseOauthStatus({ state: 'connected', address: 'alex@example.com' })).toEqual({
      state: 'connected', address: 'alex@example.com', hint: undefined, code: undefined, adminConsentUrl: undefined,
    })
  })

  it('refuses {}, a list and an unknown state', () => {
    expect(() => parseOauthStatus({})).toThrow(MailReplyError)
    expect(() => parseOauthStatus([])).toThrow(MailReplyError)
    expect(() => parseOauthStatus({ state: 42 })).toThrow(MailReplyError)
  })
})

describe('mailErrorInfo', () => {
  it("reads the daemon's code and hint off a DaemonHttpError", () => {
    const e = new DaemonHttpError('Gmail must be linked from the desktop app', 409, 'remote_unsupported', {
      code: 'remote_unsupported',
      hint: 'Gmail must be linked from the desktop app',
    })
    expect(mailErrorInfo(e)).toEqual({ code: 'remote_unsupported', hint: 'Gmail must be linked from the desktop app' })
  })

  it('still reads the JSON-in-message shape', () => {
    expect(mailErrorInfo(new Error('{"ok":false,"error":{"code":"not_built","hint":"later"}}'))).toEqual({ code: 'not_built', hint: 'later' })
  })
})
