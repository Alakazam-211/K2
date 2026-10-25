import { describe, expect, it } from 'vitest'
import { folderTrustMessage } from './folderTrustMessage'

describe('folderTrustMessage', () => {
  it('shows the refusal sentence for a held folder', () => {
    const body = JSON.stringify({
      error: 'folder_trust_pending',
      message: 'Waiting for the owner to trust /w: it has settings that run commands when programs start there.',
      decide: 'k2 workspace trust /w --yes|--always|--no (owner)',
      findings: [{ file: '.claude/settings.json', what: 'hooks' }],
    })
    expect(folderTrustMessage(409, body)).toBe(
      'Waiting for the owner to trust /w: it has settings that run commands when programs start there.',
    )
  })

  it('falls back to the decide hint when there is no message', () => {
    const body = JSON.stringify({ error: 'folder_trust_denied', decide: 'k2 workspace trust /w --yes (owner)' })
    expect(folderTrustMessage(409, body)).toBe(
      'K2 won\'t start programs in this folder: the owner chose "Don\'t trust". k2 workspace trust /w --yes (owner)',
    )
  })

  it('leaves other bodies and statuses alone', () => {
    expect(folderTrustMessage(409, JSON.stringify({ error: 'tab_closed' }))).toBeNull()
    expect(folderTrustMessage(409, 'not json')).toBeNull()
    expect(folderTrustMessage(400, JSON.stringify({ error: 'folder_trust_pending', message: 'x' }))).toBeNull()
  })
})
