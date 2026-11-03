//! The scaffold for rows only the phone door answers: the catalog's list and
//! the page these rows read. Fail loudly.

use super::*;
use crate::garden_data::call::door_tests::door;

#[test]
fn the_page_a_row_reads_is_the_resolved_page_s_layout_and_widgets() {
    let d = door(&["store:read"]);
    let page = d.f.page_of(&d.garden).expect("page");
    let resolved = d.f.resolve(Some(&d.garden)).expect("resolve");
    assert_eq!(page["layout"], resolved["page"]["layout"]);
    let ids = |p: &serde_json::Value| -> Vec<String> {
        p["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["id"].as_str().map(str::to_string)).collect()
    };
    assert_eq!(ids(&page), ids(&resolved["page"]));
    assert_eq!(ids(&page), ["b1"]);
    assert!(d.f.page_of("g-ffffffff").is_err(), "an unknown Garden");
}
