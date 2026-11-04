//! Every code `GROUP_REFUSALS` lists is one a group can be refused with: one group each, built as a
//! host's Rust ops (`support/group_cases.rs`), and `apply_group` refuses it with exactly that code.
//! The list is where a host reads which refusals a group can give; this holds the list to the code.

#[path = "support/group_cases.rs"]
mod group_cases;

use group_cases::group_cases;
use k2_mpc_board::*;

#[test]
fn every_listed_code_is_one_a_group_is_refused_with() {
    let cases = group_cases();
    for c in &cases {
        let mut b = c.board.clone();
        match apply_group(&mut b, &c.group) {
            Err(Conflict::Refused(r)) => {
                assert_eq!(r.code, c.code, "the group made to refuse with {}", c.code)
            }
            other => panic!("{}: wanted a refused group, got {other:?}", c.code),
        }
    }
    let made: Vec<RefusalCode> = cases.iter().map(|c| c.code).collect();
    assert_eq!(
        GROUP_REFUSALS,
        made.as_slice(),
        "the list holds exactly the codes a group is refused with, in this order"
    );
}
