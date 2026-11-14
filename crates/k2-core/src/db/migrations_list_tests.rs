//! The list `build.rs` writes from `drizzle_sql/ORDER` is the list written
//! by hand in `db/mod.rs`. The runner still reads the hand list; this test
//! is what lets it read the generated one later.

include!(concat!(env!("OUT_DIR"), "/migrations_list.rs"));

#[test]
fn the_order_file_list_equals_the_hand_list() {
    let by_hand = super::MIGRATIONS_BY_HAND;
    for (at, (made, hand)) in MIGRATIONS.iter().zip(by_hand).enumerate() {
        assert_eq!(
            made.0, hand.0,
            "position {at} (from 0): drizzle_sql/ORDER has `{}`, the hand list in db/mod.rs has `{}`",
            made.0, hand.0
        );
        assert!(
            made.1 == hand.1,
            "position {at} (from 0): `{}` is in both lists but the SQL differs ({} bytes from ORDER, {} bytes by hand)",
            made.0,
            made.1.len(),
            hand.1.len()
        );
    }
    // The shared part is equal; the longer list names what the other lacks.
    assert!(
        MIGRATIONS.len() == by_hand.len(),
        "position {}: drizzle_sql/ORDER has {} migrations, the hand list in db/mod.rs has {}; first one missing from the shorter list: ORDER `{}`, hand list `{}`",
        MIGRATIONS.len().min(by_hand.len()),
        MIGRATIONS.len(),
        by_hand.len(),
        MIGRATIONS.get(by_hand.len()).map_or("(none)", |m| m.0),
        by_hand.get(MIGRATIONS.len()).map_or("(none)", |m| m.0)
    );
}
