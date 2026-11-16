//! What a preview carries besides its image, in the shapes the client gets
//! (the route's JSON, the docs socket's frames).
//!
//! - **A scene's listing** ([`scene_item`]): an object's local box
//!   (`bounds {min, max, approx}`, mesh space: the client applies the
//!   object's transform; `approx` when the box may not be where or what the
//!   file shows, and then `approx_reason` says why: `"modifiers"` when the
//!   object is drawn without its modifiers, `"driven"` when its placement is
//!   driven and not evaluated, so the box sits at its base transform;
//!   `"modifiers"` wins when both are so, the evaluated mesh being the
//!   further off), how an object hidden in the file is hidden (`hidden_by:
//!   "self"` or `"collection"`, left out when it is shown; the container's
//!   own `hidden` is then true, as for a hidden slide; a hidden object is
//!   listed with its box all the same, and only its own view draws it),
//!   a camera's display values (`camera {active, lens_mm, sensor_mm,
//!   sensor_fit}`, each left out when the file has none), and the titles
//!   the view shows ([`scene_title`]: "Scene", "Turntable 1" to "Turntable 8").
//! - **A scene view's image** ([`scene_view`]): the camera that drew it
//!   (`camera {view, proj, fx, fy, near, shift_x, shift_y}`: column-major
//!   world-to-view and an OpenGL projection with an infinite far plane), so
//!   the client can lay its own overlays over the still. Left out when the
//!   worker had none (a degenerate scene). `base_values: true` when an
//!   object it draws is shown at its base transform (the view says "shown at
//!   base values"), and `hidden_left_out`, how many objects hidden in the
//!   file the view left out (the scene, a camera, a turntable frame); each
//!   left out when there is nothing to say.
//! - **Its id table** ([`ids_meta`], the scene, object and camera views
//!   only): `ids {layer, image_len, count, objects: [{index, id}]}`. The id
//!   image itself is binary and cached beside the image: the client asks
//!   the image route for it with the same container, width, hash and look
//!   plus `layer=ids` (`layer` names that value). It is a PNG at the still's
//!   size, 8-bit RGB, each pixel `r << 16 | g << 8 | b`: 0 where nothing is
//!   drawn, else the index + 1 of the object under it, which `objects` names
//!   (only the objects in the image, ascending by index; fewer than `count`
//!   when cut short). Ids are passed through as the file has them and may
//!   repeat across indexes. The pixels hold indexes, so a rename changes
//!   only the table: the host gives a current image's table the ids the
//!   listing has now ([`current_ids`]), and when an object is renamed in
//!   place, the scene and camera views' current images, and the object's
//!   own view when the read keys it (its address stays), are pushed again
//!   with their tables named anew (`service`'s head move). A push lists the
//!   objects only up to [`PUSH_IDS_MAX`] bytes of JSON ([`ids_for_push`]);
//!   the route always has the whole table.
//!
//! None of it is a digest: raw digests stay on the host, the client sees
//! keyed `render_hash`es only (rendering-data contract Item 1).
//!
//! Numbers are f32 in the renderer; each goes out in its shortest form that
//! reads back as the same f32 (`0.1`, not `0.10000000149011612`).

use std::collections::HashMap;

use k2_mpc_base::errors::Value;
use k2_scenerender::digest::{Item, LocalBounds, Target};
use k2_scenerender::proto::{CameraMeta, IdsMeta, ViewMeta};
use k2_scenerender::scene::CameraDisplay;
use serde_json::{json, Map, Value as J};

use super::Container;

/// The image route's `layer` for an id image.
pub const IDS_LAYER: &str = "ids";

/// The most JSON a `preview_ready` push spends on an id table's objects;
/// past it the push sends the count alone (the docs socket's frames are
/// capped at 4 MiB, a table may be as long).
pub const PUSH_IDS_MAX: usize = 1 << 20;

/// An f32 as JSON, in its shortest form (`null` when it isn't finite).
pub fn num(x: f32) -> J {
    x.to_string().parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(J::Null, J::Number)
}

pub fn nums(xs: &[f32]) -> J {
    J::Array(xs.iter().copied().map(num).collect())
}

/// The title a scene container shows: the file's id for a camera or an
/// object, "Scene" for the auto view, "Turntable 1" to "Turntable 8" for the
/// turntable's frames (the listing's empty titles, named in this one place).
pub fn scene_title(target: Target, id: &str) -> String {
    match target {
        Target::Scene => "Scene".into(),
        Target::Turntable(k) => format!("Turntable {}", u32::from(k) + 1),
        Target::Camera(_) | Target::Object(_) => id.to_string(),
    }
}

/// An object's box; `driven`: its placement is not evaluated. `approx_reason`
/// only when `approx`: `"modifiers"` wins over `"driven"`.
pub fn bounds(b: &LocalBounds, driven: bool) -> J {
    let mut o = json!({ "min": nums(&b.min), "max": nums(&b.max), "approx": b.approx || driven });
    if b.approx {
        o["approx_reason"] = json!("modifiers");
    } else if driven {
        o["approx_reason"] = json!("driven");
    }
    o
}

pub fn camera_display(d: &CameraDisplay) -> J {
    let mut o = Map::new();
    if let Some(a) = d.active {
        o.insert("active".into(), J::Bool(a));
    }
    if let Some(l) = d.lens_mm {
        o.insert("lens_mm".into(), num(l));
    }
    if let Some(s) = d.sensor_mm {
        o.insert("sensor_mm".into(), nums(&s));
    }
    if let Some(f) = d.sensor_fit {
        o.insert("sensor_fit".into(), serde_json::to_value(f).expect("a sensor fit is a word"));
    }
    J::Object(o)
}

pub fn camera_meta(c: &CameraMeta) -> J {
    json!({
        "view": nums(&c.view),
        "proj": nums(&c.proj),
        "fx": num(c.fx),
        "fy": num(c.fy),
        "near": num(c.near),
        "shift_x": num(c.shift_x),
        "shift_y": num(c.shift_y),
    })
}

/// A view's id table, as the client gets it.
pub fn ids_meta(m: &IdsMeta) -> J {
    json!({
        "layer": IDS_LAYER,
        "image_len": m.image_len,
        "count": m.count,
        "objects": m.objects.iter().map(|e| json!({ "index": e.index, "id": e.id })).collect::<Vec<_>>(),
    })
}

/// An id table as a push carries it: whole when its JSON is at most
/// [`PUSH_IDS_MAX`] bytes, else with no objects listed (the count says it
/// is cut short; the route's answer has them).
pub fn ids_for_push(ids: &J) -> J {
    let listed = ids.get("objects").map_or(0, |o| serde_json::to_vec(o).map_or(usize::MAX, |b| b.len()));
    let mut v = ids.clone();
    if listed > PUSH_IDS_MAX {
        v["objects"] = json!([]);
    }
    v
}

/// Give an id table (`ids`, of the image drawn for `view`) the ids the
/// objects have in `listing` now. An object view shows only its object, so
/// each entry is named as `view` is now (its index stays: the pixels hold
/// it, and an object's view keeps its render hash when objects before it
/// come or go). Every other view draws all the objects in order, and its
/// render hash covers that order, so for its current image an index is
/// still the same object: each entry takes the listed id of the object at
/// its index. Only for an image at the view's current render hash.
pub fn current_ids<'a>(ids: &mut J, view: &Container, listing: impl IntoIterator<Item = &'a Container>) {
    let Some(objects) = ids.get_mut("objects").and_then(J::as_array_mut) else { return };
    let own = match Target::from_locator(&view.locator) {
        Some(Target::Object(_)) => Some(view.title.as_str()),
        Some(_) => None,
        None => return,
    };
    let by_index = match own {
        Some(_) => HashMap::new(),
        None => object_ids(listing),
    };
    for e in objects {
        let index = e.get("index").and_then(J::as_u64).and_then(|i| u32::try_from(i).ok());
        if let Some(id) = own.or_else(|| index.and_then(|i| by_index.get(&i).copied())) {
            e["id"] = json!(id);
        }
    }
}

/// The listed id of each object of a scene, by its index (empty for a
/// listing without objects, a deck's).
pub fn object_ids<'a>(listing: impl IntoIterator<Item = &'a Container>) -> HashMap<u32, &'a str> {
    listing
        .into_iter()
        .filter_map(|c| match Target::from_locator(&c.locator) {
            Some(Target::Object(i)) => Some((i, c.title.as_str())),
            _ => None,
        })
        .collect()
}

/// A scene listing item's extras: `bounds` for an object with a box and
/// `hidden_by` for one hidden in the file (not `hidden`: the container's own
/// flag has that name), `camera` (display values) for a camera, nothing else.
pub fn scene_item(it: &Item) -> Map<String, J> {
    let mut o = Map::new();
    if let Some(b) = &it.bounds {
        o.insert("bounds".into(), bounds(b, it.not_evaluated));
    }
    if let Some(h) = it.hidden {
        o.insert("hidden_by".into(), json!(h.as_str()));
    }
    if let Some(d) = &it.camera {
        o.insert("camera".into(), camera_display(d));
    }
    o
}

/// A drawn scene view's extras: the `camera` that drew it, when the worker
/// had one, then its id table (`ids`), when it was drawn with one, then
/// `base_values` and `hidden_left_out` when there is something to say.
pub fn scene_view(v: &ViewMeta) -> Map<String, J> {
    let mut o = Map::new();
    if let Some(c) = v.camera.as_ref().filter(|c| c.is_finite()) {
        o.insert("camera".into(), camera_meta(c));
    }
    if let Some(m) = &v.ids {
        o.insert("ids".into(), ids_meta(m));
    }
    if v.base_values {
        o.insert("base_values".into(), J::Bool(true));
    }
    if v.hidden_left_out > 0 {
        o.insert("hidden_left_out".into(), json!(v.hidden_left_out));
    }
    o
}

/// A JSON value as the docs socket's frames carry it (a number that isn't
/// finite can't be in either).
pub fn to_wire(v: &J) -> Value {
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => {
            if let Some(u) = n.as_u64() {
                Value::UInt(u)
            } else if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                n.as_f64().and_then(Value::float).unwrap_or(Value::Null)
            }
        }
        J::String(s) => Value::Str(s.clone()),
        J::Array(a) => Value::List(a.iter().map(to_wire).collect()),
        J::Object(o) => Value::Obj(o.iter().map(|(k, v)| (k.clone(), to_wire(v))).collect()),
    }
}

/// A map's entries as frame fields, in order.
pub fn wire_fields(m: &Map<String, J>) -> Vec<(String, Value)> {
    m.iter().map(|(k, v)| (k.clone(), to_wire(v))).collect()
}
