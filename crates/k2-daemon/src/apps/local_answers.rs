//! One per-App answerer for the requests K2 answers itself on an App's
//! hosts (prd-tauri-core-0467-v1 TC-9/TC-22, vs-live TA-9).
//!
//! The App router asks this module, never one feature: today the App's
//! pairing link files (`app_pairing::links`) and the update feed's answers
//! (`/_k2/updates/…`, `updates_serve`). The links setting (TC-20) plugs in
//! here too. Each part keeps its own rules (which paths, which hosts, which
//! switch); this module only composes them, first answer wins.
//!
//! `None` means no part answers anything for the App now: the router then
//! splices without reading the first request (no peek, no local answers),
//! exactly as before any part existed.

use std::sync::Arc;

use crate::client_ip::LocalAnswer;

/// Compose `parts` into one answerer (`None` when there are none).
pub fn compose(parts: Vec<LocalAnswer>) -> Option<LocalAnswer> {
    match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ => Some(Arc::new(move |method: &str, path: &str, host: Option<&str>| parts.iter().find_map(|a| a(method, path, host)))),
    }
}

/// K2's own answers for the runtime App `app_id` (its label), or `None`.
pub fn for_app(app_id: &str) -> Option<LocalAnswer> {
    compose([crate::app_pairing::links::local_for(app_id), super::updates_serve::local_for(app_id)].into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(path: &'static str, body: &'static str) -> LocalAnswer {
        Arc::new(move |_m: &str, p: &str, _h: Option<&str>| (p == path).then(|| body.as_bytes().to_vec()))
    }

    #[test]
    fn no_parts_is_no_answerer() {
        assert!(compose(Vec::new()).is_none());
    }

    #[test]
    fn the_first_part_that_answers_wins_and_misses_pass_through() {
        let a = compose(vec![fixed("/a", "one"), fixed("/b", "two"), fixed("/a", "shadowed")]).expect("answerer");
        assert_eq!(a("GET", "/a", None), Some(b"one".to_vec()));
        assert_eq!(a("GET", "/b", None), Some(b"two".to_vec()));
        assert_eq!(a("GET", "/c", None), None, "a path no part knows goes to the App");
    }

    #[test]
    fn one_part_is_that_part() {
        let a = compose(vec![fixed("/x", "x")]).expect("answerer");
        assert_eq!(a("GET", "/x", Some("h")), Some(b"x".to_vec()));
    }
}
