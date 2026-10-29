import { describe, expect, it } from 'vitest'
import { duplicateConversationMessage, folderTrustMessage } from './folderTrustMessage'

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

  it('does not claim a duplicate-conversation refusal', () => {
    expect(folderTrustMessage(409, JSON.stringify({ error: 'conversation_is_canonical', message: 'x' }))).toBeNull()
  })

  it('leaves other bodies and statuses alone', () => {
    expect(folderTrustMessage(409, JSON.stringify({ error: 'tab_closed' }))).toBeNull()
    expect(folderTrustMessage(409, 'not json')).toBeNull()
    expect(folderTrustMessage(400, JSON.stringify({ error: 'folder_trust_pending', message: 'x' }))).toBeNull()
  })
})

describe('duplicateConversationMessage', () => {
  it("shows the daemon's sentence for a refused duplicate", () => {
    const body = JSON.stringify({
      error: 'conversation_is_canonical',
      message: 'This conversation is already running in another tab, so switch to that tab instead of opening a second copy.',
      conversationId: 'c-1',
    })
    expect(duplicateConversationMessage(409, body)).toBe(
      'This conversation is already running in another tab, so switch to that tab instead of opening a second copy.',
    )
  })

  it('falls back to the Chat tab sentence when the body has no message', () => {
    expect(duplicateConversationMessage(409, JSON.stringify({ error: 'conversation_is_canonical' }))).toBe(
      "This is the agent's own conversation, so open it from the agent's Chat tab instead of a new tab.",
    )
  })

  it('leaves other bodies and statuses alone', () => {
    expect(duplicateConversationMessage(409, JSON.stringify({ error: 'folder_trust_pending', message: 'x' }))).toBeNull()
    expect(duplicateConversationMessage(409, 'not json')).toBeNull()
    expect(duplicateConversationMessage(400, JSON.stringify({ error: 'conversation_is_canonical', message: 'x' }))).toBeNull()
  })
})
