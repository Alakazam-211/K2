//! What a preview carries besides its image, in the shapes the client gets
//! (the route's JSON, the docs socket's frames).
//!
//! - **A scene's listing** ([`scene_item`]): an object's local box
//!   (`bounds {min, max, approx}`, mesh space: the client applies the
//!   object's transform; `approx` when it is drawn without its modifiers),
//!   a camera's display values (`camera {active, lens_mm, sensor_mm,
//!   sensor_fit}`, each left out when the file has none), and the titles
//!   the view shows ([`scene_title`]: "Scene", "Turntable 1" to "Turntable 8").
//! - **A scene view's image** ([`scene_view`]): the camera that drew it
//!   (`camera {view, proj, fx, fy, near, shift_x, shift_y}`: column-major
//!   world-to-view and an OpenGL projection with an infinite far plane), so
//!   the client can lay its own overlays over the still. Left out when the
//!   worker had none (a degenerate scene).
//!
//! None of it is a digest: raw digests stay on the host, the client sees
//! keyed `render_hash`es only (rendering-data contract Item 1).
//!
//! Numbers are f32 in the renderer; each goes out in its shortest form that
//! reads back as the same f32 (`0.1`, not `0.10000000149011612`).

use k2_mpc_base::errors::Value;
use k2_scenerender::digest::{Item, LocalBounds, Target};
use k2_scenerender::proto::{CameraMeta, ViewMeta};
use k2_scenerender::scene::CameraDisplay;
use serde_json::{json, Map, Value as J};

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

pub fn bounds(b: &LocalBounds) -> J {
    json!({ "min": nums(&b.min), "max": nums(&b.max), "approx": b.approx })
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

/// A scene listing item's extras: `bounds` for an object with a box,
/// `camera` (display values) for a camera, nothing else.
pub fn scene_item(it: &Item) -> Map<String, J> {
    let mut o = Map::new();
    if let Some(b) = &it.bounds {
        o.insert("bounds".into(), bounds(b));
    }
    if let Some(d) = &it.camera {
        o.insert("camera".into(), camera_display(d));
    }
    o
}

/// A drawn scene view's extras: the `camera` that drew it, when the worker
/// had one.
pub fn scene_view(v: &ViewMeta) -> Map<String, J> {
    let mut o = Map::new();
    if let Some(c) = v.camera.as_ref().filter(|c| c.is_finite()) {
        o.insert("camera".into(), camera_meta(c));
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
