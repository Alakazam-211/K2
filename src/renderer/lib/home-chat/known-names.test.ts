// @vitest-environment jsdom
// The last name K2 saw for each person on a server, so a DM whose person
// has left (no longer on the server's people list) is still headed by their
// name, never "this person" or "Someone".
import { beforeEach, describe, expect, it } from 'vitest'
import { formerPersonName, rememberPeopleNames, resetKnownNamesForTests } from './known-names'

const people = (list: Array<{ ext: string; memberId: string | null; displayName: string; username: string }>) => ({
  me: { ext: 'k2:connect:me', memberId: '1' },
  people: list.map((p) => ({ ...p, picture: null, role: null, ready: true })),
})

beforeEach(() => resetKnownNamesForTests())

describe('known names', () => {
  it('finds a person by their address once their people list was seen', () => {
    rememberPeopleNames('local', people([{ ext: 'k2:connect:a', memberId: '100', displayName: 'Ana Ruiz', username: 'ana' }]))
    // They leave: the next list has no Ana.
    rememberPeopleNames('local', people([]))
    expect(formerPersonName('local', 'k2:connect:a', [], '1')).toBe('Ana Ruiz')
  })

  it('else by the conversation\'s other author', () => {
    rememberPeopleNames('local', people([{ ext: 'k2:connect:old', memberId: '100', displayName: 'Ana Ruiz', username: 'ana' }]))
    expect(formerPersonName('local', 'k2:connect:a', ['1', '100'], '1')).toBe('Ana Ruiz')
  })

  it('keeps servers apart, falls back to the username, and is null when unknown', () => {
    rememberPeopleNames('acme', people([{ ext: 'k2:connect:b', memberId: '200', displayName: '', username: 'ben' }]))
    expect(formerPersonName('acme', 'k2:connect:b', [], '1')).toBe('ben')
    expect(formerPersonName('local', 'k2:connect:b', [], '1')).toBeNull()
    expect(formerPersonName('local', 'k2:connect:zz', ['1'], '1')).toBeNull()
  })

  it('survives a reload (kept in this window\'s storage)', () => {
    rememberPeopleNames('local', people([{ ext: 'k2:connect:a', memberId: '100', displayName: 'Ana Ruiz', username: 'ana' }]))
    resetKnownNamesForTests({ keepStorage: true })
    expect(formerPersonName('local', 'k2:connect:a', [], '1')).toBe('Ana Ruiz')
  })
})
