import { describe, expect, it } from 'vitest'
import { asNameMap, nameInitials, parsePersonNames, personDisplayName, personTooltip, realName } from './person-name'

describe('person names (display only)', () => {
  it('realName: "First Last", either alone, trimmed; null when neither', () => {
    expect(realName({ firstName: 'Rosson', lastName: 'Long' })).toBe('Rosson Long')
    expect(realName({ firstName: ' Cher ', lastName: null })).toBe('Cher')
    expect(realName({ firstName: null, lastName: 'Long' })).toBe('Long')
    expect(realName({ firstName: '  ', lastName: '' })).toBeNull()
    expect(realName(null)).toBeNull()
  })

  it('personDisplayName: the real name, else the daemon displayName, else the username ("Owner" for owner)', () => {
    expect(personDisplayName('z3thon', { firstName: 'Rosson', lastName: 'Long' })).toBe('Rosson Long')
    expect(personDisplayName('z3thon', { displayName: 'z3thon' })).toBe('z3thon')
    expect(personDisplayName('z3thon')).toBe('z3thon')
    expect(personDisplayName('owner')).toBe('Owner')
  })

  it('initials come from the first and last name', () => {
    expect(nameInitials({ firstName: 'rosson', lastName: 'long' })).toBe('RL')
    expect(nameInitials({ firstName: 'Cher' })).toBe('C')
    expect(nameInitials({ lastName: 'Long' })).toBe('L')
    expect(nameInitials({})).toBeNull()
  })

  it('tooltip: "First Last (username) · role", else "username · role"', () => {
    expect(personTooltip('z3thon', 'admin', { firstName: 'Rosson', lastName: 'Long' })).toBe('Rosson Long (z3thon) · admin')
    expect(personTooltip('owner', 'owner', null)).toBe('Owner · owner')
    expect(personTooltip('anna', 'member', null, ' (you)')).toBe('anna (you) · member')
  })

  it('parses wire entries and maps, dropping junk', () => {
    expect(parsePersonNames({ firstName: 'A', lastName: '', displayName: 'A' })).toEqual({ firstName: 'A', lastName: null, displayName: 'A' })
    expect(asNameMap({ owner: { firstName: 'R' }, bad: 3 })).toEqual({ owner: { firstName: 'R', lastName: null, displayName: null } })
    expect(asNameMap(null)).toEqual({})
    expect(asNameMap([1])).toEqual({})
  })
})
