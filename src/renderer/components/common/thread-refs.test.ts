// thread-reply-to-v1: which `#N` in a message is a reference, and what the
// resolver says about each (only a message or card of this Thread links).
import { describe, expect, it, vi } from 'vitest'
import { findThreadRefs, ThreadRefResolver } from './thread-refs'

const seqs = (text: string): number[] => findThreadRefs(text).map((r) => r.seq)

describe('findThreadRefs', () => {
  it('takes #N standing alone, with its span', () => {
    expect(findThreadRefs('card #13664, ok')).toEqual([{ start: 5, end: 11, seq: 13664 }])
    expect(seqs('#1 (#2) [#3] "#4". #5!')).toEqual([1, 2, 3, 4, 5])
  })
  it('never a word#N, &#N, ##N, a path or a #N followed by letters', () => {
    expect(seqs('GH#22 a&#39; ##4 /x#5 \\#6 #7th #8_ #0')).toEqual([])
  })
})

describe('ThreadRefResolver', () => {
  it('knows what is loaded; asks the server only below the oldest while more exists', async () => {
    const probe = vi.fn(async (seq: number) => seq === 4)
    const r = new ThreadRefResolver(probe)
    r.setLoaded([10, 11, 13], true)
    expect([r.resolves(11), r.resolves(12), r.resolves(20)]).toEqual([true, false, false])
    expect(r.resolves(4)).toBe('ask')
    r.request(4)
    r.request(5)
    r.request(12)
    expect(r.resolves(4)).toBe('pending')
    await Promise.resolve()
    await Promise.resolve()
    expect([r.resolves(4), r.resolves(5)]).toEqual([true, false])
    expect(probe.mock.calls.map((c) => c[0])).toEqual([4, 5])
    r.setLoaded([10, 11, 13], false)
    expect(r.resolves(3)).toBe(false)
  })
  it('tells its listeners when what is loaded changes', () => {
    const r = new ThreadRefResolver(null)
    const fn = vi.fn()
    r.subscribe(fn)
    r.setLoaded([1], false)
    r.setLoaded([1], false)
    r.setLoaded([1, 2], false)
    expect(fn).toHaveBeenCalledTimes(2)
    expect(r.resolves(2)).toBe(true)
  })
})
