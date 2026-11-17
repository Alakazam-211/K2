import { describe, expect, it } from 'vitest'
import { callPersonName, dmCallRoomName, type CallNamesSource } from './call-names'

const ana = { ext: 'k2:connect:ana', username: 'ana', displayName: 'Ana Ruiz', picture: null, role: null, ready: true, memberId: '101' }
const SRC: CallNamesSource = {
  people: { me: { ext: 'k2:owner', memberId: '100' }, people: [ana] } as CallNamesSource['people'],
  summary: { chat: 'live', convs: [{ conv: '900', kind: 'dm', unread: false, badge: { kind: 'none' }, lastAt: null, lastMessageId: null, with: 'k2:connect:ana' }] } as CallNamesSource['summary'],
  dms: {},
}

describe('DM call names', () => {
  it("names a call's person by their member id", () => {
    expect(callPersonName(SRC, '101')).toBe('Ana Ruiz')
    expect(callPersonName(SRC, '999')).toBeNull()
    expect(callPersonName(undefined, '101')).toBeNull()
  })

  it("names a DM call by the other person, from the summary or this window's DMs", () => {
    expect(dmCallRoomName(SRC, 'dm:900')).toBe('Ana Ruiz')
    expect(dmCallRoomName({ ...SRC, summary: null, dms: { 'k2:connect:ana': '900' } }, 'dm:900')).toBe('Ana Ruiz')
    expect(dmCallRoomName(SRC, 'room-1')).toBeNull()
    expect(dmCallRoomName({ ...SRC, people: null }, 'dm:900')).toBeNull()
  })
})
