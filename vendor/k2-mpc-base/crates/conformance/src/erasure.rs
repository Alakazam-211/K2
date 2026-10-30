//! Rules over a doc's history: [`HistoryPurge`] and [`Erasure`] (doc level).
//!
//! - `erase.versions` (E-ERASE-7, E-HIST-1): `show` and `restore` of a version that held
//!   purged content give that version minus the purged spans, and restore never brings
//!   purged content back; a purge reaches live text, deleted text, text typed in two
//!   batches, and every version, and spares everything else.
//! - `erase.participant` (E-ERASE-5): erasing a participant replaces them by a pseudonym in
//!   attribution (blame, the log), keeps the counts and the live text, erases the content
//!   of their changes, and leaves no byte of their identifier.
//!
//! Every byte scan has its planted negative: the same scan finds the needle before the
//! purge. Secrets are random per run ([`secret`]).

use crate::caps::{Erasure, HistoryPurge};
use crate::ins;
use crate::versions::{edit, engine, restore, show, swap};

/// Every rule over [`HistoryPurge`], by id.
pub const HISTORY_RULES: &[&str] = &["erase.versions"];

/// Every rule over [`Erasure`], by id.
pub const PARTICIPANT_RULES: &[&str] = &["erase.participant"];

/// Run rule `id` on engine `H`. An unknown id panics.
pub fn run_history<H: HistoryPurge>(id: &str) {
    match id {
        "erase.versions" => versions::<H>(),
        _ => panic!("no history purge rule {id:?}"),
    }
}

/// Run rule `id` on engine `H`. An unknown id panics.
pub fn run_participant<H: Erasure>(id: &str) {
    match id {
        "erase.participant" => participant::<H>(),
        _ => panic!("no erasure rule {id:?}"),
    }
}

const OWNER: u64 = 100;
const A: u64 = 101;
const B: u64 = 102;
const R: u64 = 103;
const X: u64 = 107;

/// A random 32-byte secret (hex), new on every call: a byte scan for it can only find what
/// a rule planted.
pub fn secret() -> String {
    let b: [u8; 16] = rand::random();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn purge<H: HistoryPurge>(d: &mut H, needle: &str) {
    if let Err(e) = d.purge_text(OWNER, needle) {
        panic!("{}: purge refused: {e:?}", engine::<H>());
    }
}

/// `erase.versions`
pub fn versions<H: HistoryPurge>() {
    let mut d = H::open("head\n");
    let e = engine::<H>();
    let (s, k) = (secret(), secret());

    // s typed by a, k (kept) by b; v1 holds both; then a deletes s's line
    let t = d.text();
    edit(&mut d, A, vec![ins(t.len(), &format!("a: {s}\n"))]);
    let t = d.text();
    edit(
        &mut d,
        B,
        vec![ins(t.len(), &format!("keep: {k}\nb é🎉\n"))],
    );
    let v1 = d.version(A);
    let t1 = d.text();
    swap(&mut d, A, &format!("a: {s}\n"), "");
    let v2 = d.version(B);
    let t2 = d.text();
    assert!(!t2.contains(&s), "{e}");
    // the planted negative: the scan finds both
    assert!(d.holds(&s), "{e}: the scan misses a secret in history");
    assert!(d.holds(&k), "{e}: the scan misses a live secret");

    // purge s (deleted text): nothing live changes; every copy goes; k stays
    purge(&mut d, &s);
    assert_eq!(
        d.text(),
        t2,
        "{e}: purging deleted text changed the live text"
    );
    assert!(!d.holds(&s), "{e}: purged content is still held");
    assert!(d.holds(&k), "{e}: the purge erased other content");
    let t1_minus = t1.replace(&s, "");
    assert_eq!(
        show(&d, &v1),
        t1_minus,
        "{e}: a version minus the purged span"
    );
    assert_eq!(show(&d, &v2), t2, "{e}: a version without it is unchanged");
    // restore never brings it back
    restore(&mut d, R, &v1);
    assert_eq!(d.text(), t1_minus, "{e}: restore of a purged version");
    assert!(!d.holds(&s), "{e}: a restore re-admitted purged content");

    // a live secret, in a version: purged from the text and the version
    let s2 = secret();
    swap(&mut d, B, "b é🎉", &format!("b é🎉 {s2}"));
    let v3 = d.version(B);
    let t3 = d.text();
    assert!(d.holds(&s2), "{e}");
    purge(&mut d, &s2);
    assert_eq!(d.text(), t3.replace(&s2, ""), "{e}: a live purge");
    assert_eq!(show(&d, &v3), t3.replace(&s2, ""), "{e}");
    assert!(!d.holds(&s2), "{e}: a live purge left content");

    // a secret typed in two batches (whole on screen), then deleted: no half survives
    let s3 = secret();
    let (h1, h2) = s3.split_at(s3.len() / 2);
    let t = d.text();
    edit(&mut d, A, vec![ins(t.len(), &format!("x: {h1}\n"))]);
    let t = d.text();
    let at = t.len() - 1;
    edit(&mut d, A, vec![ins(at, h2)]);
    assert!(d.text().contains(&format!("x: {s3}\n")), "{e}");
    let v4 = d.version(A);
    swap(&mut d, B, &format!("x: {s3}\n"), "");
    assert!(
        d.holds(h1) && d.holds(h2),
        "{e}: the split planted negative"
    );
    purge(&mut d, &s3);
    assert!(
        !d.holds(h1) && !d.holds(h2),
        "{e}: half of a split secret survived"
    );
    assert!(!show(&d, &v4).contains(h1), "{e}");
    assert!(d.holds(&k), "{e}");
    // every version, still without them
    for v in [&v1, &v2, &v3, &v4] {
        let t = show(&d, v);
        for x in [&s, &s2, &s3] {
            assert!(
                !t.contains(x.as_str()),
                "{e}: version {v:?} shows a purged secret"
            );
        }
    }
}

/// `erase.participant`
pub fn participant<H: Erasure>() {
    let mut d = H::open("head\n");
    let e = engine::<H>();
    let s = secret();
    let pseudonym = format!("erased-{}", &secret()[..8]);

    // x types a secret and deletes it (history only), then a line that stays
    let t = d.text();
    edit(&mut d, X, vec![ins(t.len(), &format!("x: {s}\n"))]);
    swap(&mut d, X, &format!("x: {s}\n"), "x kept\n");
    let t = d.text();
    edit(&mut d, A, vec![ins(t.len(), "a line\n")]);
    d.version(A);
    let live = d.text();
    let label = d.label(X);
    let by_x = d.changes_by(&label);
    let authors = d.line_authors();
    assert_eq!(by_x, 2, "{e}");
    assert!(
        authors
            .iter()
            .any(|(l, who)| l == "x kept" && *who == label),
        "{e}: {authors:?}"
    );
    // the planted negative: x's identifier and secret are found
    assert!(d.holds(&label), "{e}: the scan misses the participant");
    assert!(d.holds(&s), "{e}: the scan misses the secret");

    if let Err(r) = d.erase(OWNER, X, &pseudonym) {
        panic!("{e}: erasure refused: {r:?}");
    }
    assert_eq!(d.text(), live, "{e}: erasure changed the live text");
    let after = d.line_authors();
    let want: Vec<(String, String)> = authors
        .into_iter()
        .map(|(l, who)| {
            let who = if who == label { pseudonym.clone() } else { who };
            (l, who)
        })
        .collect();
    assert_eq!(after, want, "{e}: blame after erasure");
    assert_eq!(d.changes_by(&pseudonym), by_x, "{e}: counts kept");
    assert_eq!(d.changes_by(&label), 0, "{e}");
    assert!(
        !d.holds(&label),
        "{e}: the participant's identifier is still held"
    );
    assert!(!d.holds(&s), "{e}: their content is still held");
    // the doc works on: others edit, the pseudonym keeps its lines
    swap(&mut d, B, "a line", "a line, b");
    assert_eq!(
        d.line_authors()
            .iter()
            .find(|(l, _)| l == "x kept")
            .map(|(_, w)| w.clone()),
        Some(pseudonym),
        "{e}"
    );
}
