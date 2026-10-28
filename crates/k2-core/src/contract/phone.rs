//! The catalog's `phone` column (prd-gardens-on-phone-v1 §4.3, vs-live
//! GP-A5): where each widget verb runs when its Garden is on a paired
//! phone (K2 Companion).
//!
//! - `host`: the phone's frame host answers it (page verbs);
//! - `door`: the phone forwards it to the daemon's remote door
//!   (`POST /cli/zen/remote/call`, or `/cli/zen/remote/blob/*` for bytes);
//! - `later`: not in this release (`verb_unavailable` with the row's feature);
//! - `never`: never on a phone (`not_exposed`).
//!
//! Every row exposed to `widget` or `garden-local` carries it; no other row
//! does ([`lint_phone`]). [`phone_verbs_json`] is the table the Companion
//! takes (`sdk/generated/phone-verbs.json`, written by `contract-gen`), and
//! [`phone_features`] is the hello's `features` on a phone: the desktop
//! frame's list minus every feature whose rows are all `later`/`never`
//! there, computed from this column, never hand-listed.

use super::{Catalog, Exposure, Phone, VerbRow};

/// The generated table, relative to the repo root.
pub const PHONE_VERBS_JSON: &str = "sdk/generated/phone-verbs.json";

/// True when `v` is a widget verb (exposed to `widget` or `garden-local`):
/// exactly the rows that carry `phone`.
pub fn is_widget_row(v: &VerbRow) -> bool {
    v.exposed_to(Exposure::Widget) || v.exposed_to(Exposure::GardenLocal)
}

/// `phone` on every widget row, and on no other row.
pub fn lint_phone(c: &Catalog, out: &mut Vec<String>) {
    for v in &c.verbs {
        match (is_widget_row(v), v.phone) {
            (true, None) => out.push(format!(
                "{}: a widget row needs phone (host, door, later or never; prd-gardens-on-phone-v1 §4.3)",
                v.verb
            )),
            (false, Some(_)) => out.push(format!("{}: phone is only for rows exposed to widget or garden-local", v.verb)),
            _ => {}
        }
        // A `door` row reaches the daemon: a garden-local row (the call
        // runner) or one of the byte verbs (the remote blob routes).
        if v.phone == Some(Phone::Door) && !v.exposed_to(Exposure::GardenLocal) && !BYTE_VERBS.contains(&v.verb.as_str()) {
            out.push(format!("{}: phone door needs a daemon runner (a garden-local row or a byte verb)", v.verb));
        }
    }
}

/// The byte verbs: `door` rows that move bytes over the remote blob routes
/// instead of `remote/call`.
pub const BYTE_VERBS: &[&str] = &["blobs.put", "blobs.get"];

/// Where `verb` runs on a phone; `None` for a verb that isn't a widget row
/// (the remote door answers it `not_exposed`).
pub fn phone_of(c: &Catalog, verb: &str) -> Option<Phone> {
    c.verb(verb).filter(|v| is_widget_row(v)).and_then(|v| v.phone)
}

/// The hello's `features` on a phone: `desktop` in its order, minus every
/// feature whose widget rows are all `later` or `never` there. A feature no
/// widget row names stays (it describes the frame, not a verb).
pub fn phone_features<'a>(c: &Catalog, desktop: &[&'a str]) -> Vec<&'a str> {
    desktop
        .iter()
        .copied()
        .filter(|f| {
            let rows: Vec<&VerbRow> = c.verbs.iter().filter(|v| is_widget_row(v) && v.feature == *f).collect();
            rows.is_empty() || rows.iter().any(|v| matches!(v.phone, Some(Phone::Host) | Some(Phone::Door)))
        })
        .collect()
}

/// `sdk/generated/phone-verbs.json`: `[{verb, phone, feature, cap}]`, one
/// line per widget row, sorted by verb.
pub fn phone_verbs_json(c: &Catalog) -> String {
    let mut rows: Vec<&VerbRow> = c.verbs.iter().filter(|v| is_widget_row(v)).collect();
    rows.sort_by(|a, b| a.verb.cmp(&b.verb));
    let lines: Vec<String> = rows
        .iter()
        .map(|v| {
            serde_json::json!({
                "verb": v.verb,
                "phone": v.phone.map(Phone::as_str),
                "feature": v.feature,
                "cap": v.cap,
            })
            .to_string()
        })
        .collect();
    format!(
        "{{\n  \"generatedBy\": \"contract-gen\",\n  \"catalogVersion\": {},\n  \"verbs\": [\n    {}\n  ]\n}}\n",
        c.catalog_version,
        lines.join(",\n    ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{catalog, parse, CATALOG_JSON};

    #[test]
    fn every_widget_row_has_a_phone_and_no_other_row_does() {
        let c = catalog();
        let mut out = Vec::new();
        lint_phone(c, &mut out);
        assert!(out.is_empty(), "{out:#?}");
        let n = c.verbs.iter().filter(|v| is_widget_row(v)).count();
        assert!(n >= 81, "the widget rows: {n}");
        // A seeded bad row fails each rule.
        let mut bad = parse(CATALOG_JSON).expect("the catalog parses");
        let store_get = bad.verbs.iter_mut().find(|v| v.verb == "store.get").expect("store.get");
        store_get.phone = None;
        let homes = bad.verbs.iter_mut().find(|v| v.verb == "homes.list").expect("homes.list");
        homes.phone = Some(Phone::Host);
        let links = bad.verbs.iter_mut().find(|v| v.verb == "links.open").expect("links.open");
        links.phone = Some(Phone::Door);
        let mut out = Vec::new();
        lint_phone(&bad, &mut out);
        assert_eq!(out.len(), 3, "{out:#?}");
        assert!(out.iter().any(|l| l.starts_with("store.get: a widget row needs phone")), "{out:#?}");
        assert!(out.iter().any(|l| l.starts_with("homes.list: phone is only")), "{out:#?}");
        assert!(out.iter().any(|l| l.starts_with("links.open: phone door needs")), "{out:#?}");
    }

    /// v1 (0.46.6): Garden data and person rows go through the door, the
    /// page verbs are the host's, agents/Thread/sources wait for GP3a,
    /// `guests.setRooms` waits for the phone's confirm flow.
    #[test]
    fn the_v1_table_names_what_the_prd_says() {
        let c = catalog();
        let p = |v: &str| phone_of(c, v).unwrap_or_else(|| panic!("{v} has no phone"));
        for v in ["store.get", "store.put", "save.set", "viewer.get", "widget.self", "blobs.put", "blobs.get", "tickets.list", "guests.rooms"] {
            assert_eq!(p(v), Phone::Door, "{v}");
        }
        for v in ["gardens.switch", "theme.get", "layout.set", "conversation.open", "agents.show", "canvas.setHitRegions"] {
            assert_eq!(p(v), Phone::Host, "{v}");
        }
        for v in ["guests.setRooms", "links.open", "downloads.save", "agents.list", "thread.post", "sources.read"] {
            assert_eq!(p(v), Phone::Later, "{v}");
        }
        for v in ["window.startDrag", "sources.ask"] {
            assert_eq!(p(v), Phone::Never, "{v}");
        }
        assert_eq!(phone_of(c, "homes.list"), None, "not a widget row");
        assert_eq!(phone_of(c, "no.such"), None);
    }

    #[test]
    fn phone_features_drop_the_later_only_ones() {
        let c = catalog();
        let desk = crate::zen::widget_shot::FRAME_FEATURES;
        let f = phone_features(c, desk);
        for keep in ["zen-v1", "zen-gardens-v1", "zen-widgets-v1", "garden-data-v1", "garden-blobs-v1", "guests-rooms-v1"] {
            assert!(f.contains(&keep), "{keep} stays: {f:?}");
        }
        for drop in ["sources-v1", "thread-read-v1", "thread-attachments-v1", "thread-images-v1", "widget-activity-v1", "zen-files-v1"] {
            assert!(!f.contains(&drop), "{drop} goes: {f:?}");
        }
        // Order kept.
        let order: Vec<&str> = desk.iter().copied().filter(|x| f.contains(x)).collect();
        assert_eq!(f, order);
    }

    #[test]
    fn the_generated_table_parses_and_lists_every_widget_row() {
        let c = catalog();
        let v: serde_json::Value = serde_json::from_str(&phone_verbs_json(c)).expect("phone-verbs.json is JSON");
        assert_eq!(v["catalogVersion"], c.catalog_version);
        let rows = v["verbs"].as_array().expect("verbs");
        assert_eq!(rows.len(), c.verbs.iter().filter(|r| is_widget_row(r)).count());
        let mut names: Vec<&str> = rows.iter().map(|r| r["verb"].as_str().expect("verb")).collect();
        let sorted = {
            let mut s = names.clone();
            s.sort();
            s
        };
        assert_eq!(names, sorted, "sorted by verb");
        names.dedup();
        assert_eq!(names.len(), rows.len(), "one row per verb");
        assert!(rows.iter().all(|r| ["host", "door", "later", "never"].contains(&r["phone"].as_str().unwrap_or(""))));
    }
}
