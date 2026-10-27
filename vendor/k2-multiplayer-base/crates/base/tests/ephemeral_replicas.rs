//! API invariant: an agent edit keeps no replica after it returns.
//!
//! Memory for a loaded doc must be the doc, not the doc times the number of agents that
//! touched it. A kept per-agent replica is a full copy (D0: same heap as the session), so
//! after many agents edit, the session would be many times its reloaded size.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use k2_multiplayer_base::{Doc, Kind, Schema};

const S: Schema = Schema {
    name: "test",
    roots: &[(ROOT, Kind::Text)],
};
const ROOT: &str = "source";

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size() as isize, Ordering::SeqCst);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            LIVE.fetch_add(n as isize - l.size() as isize, Ordering::SeqCst);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

fn live() -> isize {
    LIVE.load(Ordering::SeqCst)
}

#[test]
fn agent_edits_keep_no_replica() {
    // A realistic doc: 2,000 lines.
    let base: String = (0..2000)
        .map(|i| format!("- item {i}: some markdown text here\n"))
        .collect();

    let before = live();
    let mut session = Doc::new(S).unwrap();
    session.replace_text_as(2, ROOT, &base).unwrap();

    // 64 different agents each make one edit, as `k2 doc sed -i` would.
    for agent in 0..64u64 {
        let cur = session.text(ROOT).unwrap();
        let new = cur.replacen(
            &format!("item {}:", agent * 30),
            &format!("ITEM {}:", agent * 30),
            1,
        );
        assert!(session
            .replace_text_as(1000 + agent, ROOT, &new)
            .unwrap()
            .is_some());
    }
    let session_heap = live() - before;

    // The same content, reloaded: what the doc alone costs.
    let state = session.encode_state().unwrap();
    let b2 = live();
    let reloaded = Doc::load(S, &state).unwrap();
    let reloaded_heap = live() - b2;
    assert_eq!(reloaded.text(ROOT).unwrap(), session.text(ROOT).unwrap());

    // Negative control: the check below must fail if even one replica were kept.
    let b3 = live();
    let kept = Doc::load(S, &state).unwrap();
    let one_replica = live() - b3;
    assert!(
        (session_heap + one_replica) * 2 > reloaded_heap * 3,
        "the check is too loose to catch one kept replica"
    );
    drop(kept);

    // One kept replica would make the session >= 2x its reloaded size; 64 would make it ~65x.
    // Allow 1.5x for allocator slack and the live doc's own history of edits.
    assert!(
        session_heap * 2 <= reloaded_heap * 3,
        "session holds {session_heap} B after 64 agent edits but the doc alone is {reloaded_heap} B: \
         something kept per-agent state (a replica?) past edit_as"
    );
}
