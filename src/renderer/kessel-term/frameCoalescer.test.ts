import { describe, expect, it } from 'vitest'

import {
  STARVATION_FLUSH_CAP,
  createFrameCoalescer,
  type CoalescableFrame,
} from './frameCoalescer'

// Frame-budget pins for the client half of the rendering pipeline
// (pi-mono study learning B2). The daemon's 16ms emitter cadence is
// pinned in Rust (grid_emitter.rs tests); this suite pins the
// renderer's per-rAF batching so a regression that re-renders per WS
// message (the exact bug the batching was retrofitted to fix — v2
// dropped v1's scheduleRender and melted under `cat` bursts) fails
// in CI instead of shipping as "scrolling feels bad".

/** Labeled frame so tests can assert batch contents + ordering. */
interface TestFrame extends CoalescableFrame {
  id: number
}

/** Manual rAF: scheduled callbacks run only when the test says the
 *  display refreshed — cadence is fully deterministic. */
function harness() {
  const scheduled = new Map<number, () => void>()
  const applied: TestFrame[][] = []
  const cancelled: number[] = []
  let nextId = 1
  const coalescer = createFrameCoalescer<TestFrame>({
    schedule: (flush) => {
      const id = nextId++
      scheduled.set(id, flush)
      return id
    },
    cancel: (id) => {
      cancelled.push(id)
      scheduled.delete(id)
    },
    apply: (batch) => applied.push(batch),
  })
  /** Fire one animation frame: run everything currently scheduled. */
  const raf = () => {
    const cbs = [...scheduled.values()]
    scheduled.clear()
    for (const cb of cbs) cb()
  }
  return { coalescer, scheduled, applied, cancelled, raf }
}

const delta = (id: number): TestFrame => ({ kind: 'delta', id })
const snapshot = (id: number): TestFrame => ({ kind: 'snapshot', id })

describe('createFrameCoalescer', () => {
  it('applies N queued deltas as ONE batch on the next frame', () => {
    const h = harness()
    for (let i = 0; i < 5; i++) h.coalescer.enqueue(delta(i))
    // Nothing applies before the frame fires…
    expect(h.applied).toHaveLength(0)
    // …and only ONE flush was ever scheduled for the whole burst
    // (per-message scheduling would still coalesce, but a stack of
    // rAF callbacks is wasted work — pin the single-schedule shape).
    expect(h.scheduled.size).toBe(1)
    h.raf()
    // One apply — the "one setSnapshot per display refresh"
    // guarantee — carrying all five frames in arrival order.
    expect(h.applied).toHaveLength(1)
    expect(h.applied[0].map((f) => f.id)).toEqual([0, 1, 2, 3, 4])
  })

  it('a queued snapshot supersedes every frame queued before it', () => {
    const h = harness()
    h.coalescer.enqueue(delta(1))
    h.coalescer.enqueue(delta(2))
    h.coalescer.enqueue(snapshot(3))
    h.coalescer.enqueue(delta(4))
    h.raf()
    // Deltas 1 & 2 could only be merged and then discarded by the
    // snapshot's wholesale replace — they must never reach apply.
    expect(h.applied).toHaveLength(1)
    expect(h.applied[0].map((f) => f.id)).toEqual([3, 4])
  })

  it('flushes synchronously at the starvation cap, cancelling the rAF', () => {
    // rAF starves in occluded windows; the queue must not grow
    // unbounded. The cap-th enqueue flushes WITHOUT a frame firing.
    const h = harness()
    for (let i = 0; i < STARVATION_FLUSH_CAP - 1; i++) {
      h.coalescer.enqueue(delta(i))
    }
    expect(h.applied).toHaveLength(0)
    expect(h.coalescer.pendingCount()).toBe(STARVATION_FLUSH_CAP - 1)
    h.coalescer.enqueue(delta(STARVATION_FLUSH_CAP - 1))
    // Applied now — no raf() call happened.
    expect(h.applied).toHaveLength(1)
    expect(h.applied[0]).toHaveLength(STARVATION_FLUSH_CAP)
    expect(h.coalescer.pendingCount()).toBe(0)
    // The pending rAF was cancelled, so the (eventually un-starved)
    // frame must not double-apply an empty batch.
    expect(h.cancelled).toHaveLength(1)
    expect(h.scheduled.size).toBe(0)
    h.raf()
    expect(h.applied).toHaveLength(1)
  })

  it('snapshot supersede resets the starvation counter', () => {
    // A snapshot empties the queue, so a snapshot-y stream can never
    // hit the cap spuriously — only a genuine un-flushed delta
    // backlog flushes synchronously.
    const h = harness()
    for (let i = 0; i < STARVATION_FLUSH_CAP * 2; i++) {
      h.coalescer.enqueue(i % 50 === 0 ? snapshot(i) : delta(i))
    }
    expect(h.applied).toHaveLength(0)
    expect(h.coalescer.pendingCount()).toBeLessThan(STARVATION_FLUSH_CAP)
    h.raf()
    expect(h.applied).toHaveLength(1)
  })

  it('reschedules after a flush (next burst gets its own frame)', () => {
    const h = harness()
    h.coalescer.enqueue(delta(1))
    h.raf()
    expect(h.applied).toHaveLength(1)
    h.coalescer.enqueue(delta(2))
    expect(h.scheduled.size).toBe(1)
    h.raf()
    expect(h.applied).toHaveLength(2)
    expect(h.applied[1].map((f) => f.id)).toEqual([2])
  })

  it('an empty flush never calls apply', () => {
    const h = harness()
    h.coalescer.flush()
    expect(h.applied).toHaveLength(0)
    // A fired frame after clear() is the production shape of this
    // (unmount raced a scheduled flush): also a no-op.
    h.coalescer.enqueue(delta(1))
    h.coalescer.clear()
    h.raf()
    expect(h.applied).toHaveLength(0)
  })

  it('clear() cancels the scheduled flush and drops the queue', () => {
    const h = harness()
    h.coalescer.enqueue(delta(1))
    expect(h.scheduled.size).toBe(1)
    h.coalescer.clear()
    expect(h.scheduled.size).toBe(0)
    expect(h.cancelled).toHaveLength(1)
    expect(h.coalescer.pendingCount()).toBe(0)
    // Post-clear enqueue works normally (remount-after-teardown).
    h.coalescer.enqueue(delta(2))
    h.raf()
    expect(h.applied).toHaveLength(1)
    expect(h.applied[0].map((f) => f.id)).toEqual([2])
  })
})
