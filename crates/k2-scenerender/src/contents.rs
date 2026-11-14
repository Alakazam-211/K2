//! Shell and contents: which drawn objects bound the space (a floor, walls, a ceiling, a room
//! modelled as one closed mesh) and which sit inside it (furniture, props). Content framing
//! ([`crate::camera::Framing::Contents`]) fits the auto view to the contents while still drawing
//! the shell.
//!
//! **The rule** reads world-space boxes only: each object's box (its mesh's local box, the 8
//! corners through its transform, as the bounds framing uses) and the scene's box (the union of
//! every drawn object's box). An object is **shell** when either holds:
//!
//! - **a bounding surface:** it is flat (its smallest extent is at most [`FLAT_RATIO`] of its
//!   largest) and it spans the scene (its extent is at least [`SPAN_FRACTION`] of the scene's on
//!   at least two of the three axes): a floor, a ceiling, a wall, a wall-to-wall carpet;
//! - **an enclosure:** it spans the scene on all three axes and its box holds the boxes of more
//!   than half of the other objects, and of at least [`ENCLOSED_MIN`]: a room modelled as one
//!   closed mesh with the furniture inside.
//!
//! Every other object with a box is **contents**. An object whose mesh has no vertices has no
//! box ([`Class::Empty`]): it draws nothing and frames nothing.
//!
//! **Determinism:** min, max, `-`, `*` and comparisons on f64 boxes made from the f32 input, in a
//! fixed order, so the classes are the same on every machine. They depend on the set of drawn
//! objects, never on their order: the scene's box is a min/max union, and each test compares one
//! object's box with the scene's box or counts the boxes inside it.
//!
//! **Limits of the rule:** boxes are axis-aligned in world space, so a wall rotated off the axes
//! has a box that is not flat and reads as contents; a room whose walls are not one storey of
//! one space (an L-shaped plan, several rooms) may have walls shorter than [`SPAN_FRACTION`] of
//! the scene, which read as contents too. Either only widens the frame back towards the bounds
//! framing; nothing is hidden, since the shell is always drawn.

use crate::camera::object_box;
use crate::math::*;
use crate::scene::Scene;

/// Flat: the smallest of the box's three extents is at most this fraction of its largest.
///
/// A built wall is 0.1 to 0.3 m thick on 2.4 m or more of height (and a longer run), a floor
/// slab or a ceiling thinner still against its width: all at or under 1/8. A piece of furniture
/// is not that flat as a whole (a sofa, a table, a chair or a lamp is at least about a quarter as
/// deep as it is wide or tall); what is that flat inside a room (a rug, a picture, a shelf) does
/// not also span the scene. 1/8 is exact in binary, so the test is one exact product.
pub const FLAT_RATIO: f64 = 0.125;

/// Spans: the object's extent on an axis is at least this fraction of the scene's on that axis.
///
/// A bounding surface reaches across nearly all of the scene: a floor or a ceiling on both
/// horizontal axes, a wall along its run and up its height. 3/4 rather than all of it leaves
/// room for walls that stop a wall's thickness short of a corner or at a doorway, a floor inset
/// between its walls, and contents that reach a little past the walls (a lamp, a tall plant);
/// a rug or a table, however flat, covers well under 3/4 of a furnished room in both directions.
/// 3/4 is exact in binary.
pub const SPAN_FRACTION: f64 = 0.75;

/// An enclosure holds at least this many other objects' boxes (and more than half of them):
/// one box around one other object is as likely an assembly (a case and what is in it) as a room.
pub const ENCLOSED_MIN: usize = 2;

/// The enclosure test runs only when at most this many objects span the scene on all three
/// axes; with more, no object is an enclosure. Each candidate counts the boxes inside its own
/// (one pass over the objects), so this bounds the work at this many passes. A real scene has
/// one or two such objects (the room, a ground plane); many is a pile of copies, with no room
/// among them to tell apart.
pub const MAX_ENCLOSURE_CANDIDATES: usize = 64;

/// The content frame's least extent on each axis, as a fraction of the scene's largest extent:
/// contents smaller than this (one small object in a room) are framed with this much of the room
/// around them, never blown up to fill the image. 1/4 is exact in binary.
pub const MIN_FRAME_FRACTION: f64 = 0.25;

/// At most this many left-out objects are listed in a view's meta (all are counted), so a
/// response line stays bounded whatever the scene.
pub const MAX_LEFT_OUT_LISTED: usize = 1024;

/// What an object is to the framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Bounds the space: drawn, not framed.
    Shell,
    /// Inside the space: drawn and framed.
    Contents,
    /// No vertices: no box, nothing drawn.
    Empty,
}

/// A world-space box: (min, max).
pub type Box3 = (D3, D3);

/// The classes of a view's drawn objects.
#[derive(Clone, Debug, PartialEq)]
pub struct Classes {
    /// One per entry of the view's object list, in its order.
    pub class: Vec<Class>,
    /// Each entry's world box (`None`: an empty mesh).
    pub boxes: Vec<Option<Box3>>,
    /// The union of the boxes (`None`: no object has one).
    pub scene: Option<Box3>,
}

impl Classes {
    /// Is entry `k` of the view's object list shell?
    pub fn is_shell(&self, k: usize) -> bool {
        self.class[k] == Class::Shell
    }
}

fn union(a: Option<Box3>, b: Box3) -> Box3 {
    match a {
        None => b,
        Some((mut lo, mut hi)) => {
            for k in 0..3 {
                if b.0[k] < lo[k] {
                    lo[k] = b.0[k];
                }
                if b.1[k] > hi[k] {
                    hi[k] = b.1[k];
                }
            }
            (lo, hi)
        }
    }
}

fn extent((lo, hi): Box3) -> D3 {
    subd(hi, lo)
}

fn max3(e: D3) -> f64 {
    e[0].max(e[1]).max(e[2])
}

fn min3(e: D3) -> f64 {
    e[0].min(e[1]).min(e[2])
}

/// On how many axes `e` reaches [`SPAN_FRACTION`] of the scene's extent `s`.
fn spans(e: D3, s: D3) -> usize {
    (0..3).filter(|&k| e[k] >= SPAN_FRACTION * s[k]).count()
}

fn inside(inner: Box3, outer: Box3) -> bool {
    (0..3).all(|k| outer.0[k] <= inner.0[k] && inner.1[k] <= outer.1[k])
}

/// Classify the drawn objects `active` (indices into `scene.objects`) by the rule above.
pub fn classify(scene: &Scene, active: &[u32]) -> Classes {
    let mut mesh_box: Vec<Option<Option<Box3>>> = vec![None; scene.meshes.len()];
    let boxes: Vec<Option<Box3>> = active.iter().map(|&oi| object_box(scene, oi, &mut mesh_box)).collect();
    let mut acc: Option<Box3> = None;
    for b in boxes.iter().flatten() {
        acc = Some(union(acc, *b));
    }
    let mut class: Vec<Class> = boxes.iter().map(|b| if b.is_some() { Class::Contents } else { Class::Empty }).collect();
    let Some(sb) = acc else {
        return Classes { class, boxes, scene: None };
    };
    let s = extent(sb);
    let mut candidates = Vec::new();
    for (k, b) in boxes.iter().enumerate() {
        let Some(b) = *b else { continue };
        let e = extent(b);
        let n = spans(e, s);
        if min3(e) <= FLAT_RATIO * max3(e) && n >= 2 {
            class[k] = Class::Shell;
        } else if n == 3 {
            candidates.push(k);
        }
    }
    if candidates.len() <= MAX_ENCLOSURE_CANDIDATES {
        let with_box = boxes.iter().filter(|b| b.is_some()).count();
        for &k in &candidates {
            let outer = boxes[k].expect("a candidate has a box");
            let held = boxes.iter().enumerate().filter(|&(j, b)| j != k && b.is_some_and(|b| inside(b, outer))).count();
            if held >= ENCLOSED_MIN && 2 * held > with_box - 1 {
                class[k] = Class::Shell;
            }
        }
    }
    Classes { class, boxes, scene: Some(sb) }
}

/// What content framing frames.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentFrame {
    /// The box to fit: the contents' union, grown to the least frame where it is smaller.
    pub lo: D3,
    pub hi: D3,
    /// The shell objects (indices into `scene.objects`, ascending, each once): drawn, not framed.
    pub left_out: Vec<u32>,
    /// The contents were smaller than the least frame on some axis, and the box was grown.
    pub grown: bool,
}

/// The box content framing fits, or `None` when it falls back to the bounds framing: no object
/// is contents (none at all, or every one is shell), or a box isn't finite.
///
/// Where the contents' union is smaller on an axis than [`MIN_FRAME_FRACTION`] of the scene's
/// largest extent, it grows to that (never past the scene's own extent on the axis) about its
/// centre, then slides back inside the scene's box: one small object on a floor is framed with
/// some of the floor around it, not with the space under the floor.
pub fn content_frame(active: &[u32], c: &Classes) -> Option<ContentFrame> {
    let sb = c.scene?;
    let mut acc: Option<Box3> = None;
    let mut left_out = Vec::new();
    for (k, &oi) in active.iter().enumerate() {
        match (c.class[k], c.boxes[k]) {
            (Class::Contents, Some(b)) => acc = Some(union(acc, b)),
            (Class::Shell, _) => left_out.push(oi),
            _ => {}
        }
    }
    let (mut lo, mut hi) = acc?;
    let finite = |v: D3| v.iter().all(|x| x.is_finite());
    if !(finite(lo) && finite(hi) && finite(sb.0) && finite(sb.1)) {
        return None;
    }
    left_out.sort_unstable();
    left_out.dedup();
    let s = extent(sb);
    let least = MIN_FRAME_FRACTION * max3(s);
    let mut grown = false;
    for k in 0..3 {
        let e = hi[k] - lo[k];
        let want = least.min(s[k]);
        if e >= want {
            continue;
        }
        grown = true;
        let mid = 0.5 * (lo[k] + hi[k]);
        let (mut a, mut b) = (mid - 0.5 * want, mid + 0.5 * want);
        if a < sb.0[k] {
            b += sb.0[k] - a;
            a = sb.0[k];
        }
        if b > sb.1[k] {
            a = (a - (b - sb.1[k])).max(sb.0[k]);
            b = sb.1[k];
        }
        lo[k] = a;
        hi[k] = b;
    }
    Some(ContentFrame { lo, hi, left_out, grown })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgen;

    fn all(s: &Scene) -> Vec<u32> {
        (0..s.objects.len() as u32).collect()
    }

    fn shell_ids(s: &Scene) -> Vec<String> {
        let a = all(s);
        let c = classify(s, &a);
        let mut v: Vec<String> = a.iter().zip(&c.class).filter(|(_, c)| **c == Class::Shell).map(|(&i, _)| s.objects[i as usize].id.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn the_interior_scenes_floor_and_walls_are_its_shell() {
        let s = testgen::interior_scene();
        assert_eq!(shell_ids(&s), ["Floor", "Wall.Back", "Wall.Left", "Wall.Right"]);
        let a = all(&s);
        let c = classify(&s, &a);
        // The rug is flat but covers half the room each way: contents.
        let rug = s.objects.iter().position(|o| o.id == "Rug").expect("a rug");
        assert_eq!(c.class[rug], Class::Contents);
        let f = content_frame(&a, &c).expect("contents");
        assert_eq!(f.left_out, [0, 1, 2, 3]);
        assert!(!f.grown);
        // The contents' box sits inside the room's and is smaller on the floor plan.
        let (slo, shi) = c.scene.expect("a scene box");
        assert!(f.lo[0] > slo[0] && f.hi[0] < shi[0] && f.lo[2] > slo[2] && f.hi[2] < shi[2], "{f:?}");
    }

    #[test]
    fn an_empty_box_room_is_all_shell_and_frames_nothing() {
        let s = testgen::empty_room_scene();
        let a = all(&s);
        let c = classify(&s, &a);
        assert!(c.class.iter().all(|c| *c == Class::Shell), "{:?}", c.class);
        assert_eq!(content_frame(&a, &c), None);
        // No objects at all: no box, nothing to frame.
        let none = Scene { objects: Vec::new(), ..s };
        let c = classify(&none, &[]);
        assert_eq!(c.scene, None);
        assert_eq!(content_frame(&[], &c), None);
    }

    #[test]
    fn one_small_object_gets_the_least_frame_inside_the_room() {
        let s = testgen::small_object_scene();
        assert_eq!(shell_ids(&s), ["Floor", "Wall.Back", "Wall.Left", "Wall.Right"]);
        let a = all(&s);
        let c = classify(&s, &a);
        let f = content_frame(&a, &c).expect("contents");
        assert!(f.grown);
        let (slo, shi) = c.scene.expect("a scene box");
        let least = MIN_FRAME_FRACTION * max3(subd(shi, slo));
        for k in 0..3 {
            assert!(f.hi[k] - f.lo[k] >= least.min(shi[k] - slo[k]) - 1e-12, "axis {k}: {f:?}");
            assert!(f.lo[k] >= slo[k] && f.hi[k] <= shi[k], "axis {k} inside the room: {f:?}");
        }
        // The cube sits on the floor: its frame starts at the floor slab's underside, not below it.
        assert_eq!(f.lo[1], slo[1]);
        // The cube itself is inside its frame.
        let cube = c.boxes[a.len() - 1].expect("the cube's box");
        assert!(inside(cube, (f.lo, f.hi)));
    }

    #[test]
    fn the_classes_dont_depend_on_the_objects_order() {
        for s in [testgen::interior_scene(), testgen::small_object_scene(), testgen::empty_room_scene(), testgen::enclosed_scene(3)] {
            let base = shell_ids(&s);
            let a = all(&s);
            let frame = content_frame(&a, &classify(&s, &a)).map(|f| (f.lo, f.hi, f.grown));
            let n = s.objects.len();
            for rot in [1, 3, n - 1] {
                // The scene's objects reordered: reversed, then rotated.
                let mut p = s.clone();
                p.objects.reverse();
                p.objects.rotate_left(rot % n);
                assert_eq!(shell_ids(&p), base);
                let pa = all(&p);
                let pc = classify(&p, &pa);
                let pf = content_frame(&pa, &pc);
                assert_eq!(pf.as_ref().map(|f| (f.lo, f.hi, f.grown)), frame, "the same box, bit for bit");
                if let Some(pf) = pf {
                    let mut ids: Vec<&str> = pf.left_out.iter().map(|&i| p.objects[i as usize].id.as_str()).collect();
                    ids.sort();
                    assert_eq!(ids, base);
                }
                // The view's own list in another order: the same class per object.
                let shuffled: Vec<u32> = a.iter().rev().copied().collect();
                let sc = classify(&s, &shuffled);
                for (k, &oi) in shuffled.iter().enumerate() {
                    assert_eq!(sc.class[k], classify(&s, &a).class[oi as usize]);
                }
            }
        }
    }

    #[test]
    fn a_room_in_one_mesh_is_an_enclosure_when_it_holds_most_objects() {
        // One closed box around three objects: shell; around one: contents (an assembly).
        assert_eq!(shell_ids(&testgen::enclosed_scene(3)), ["Room"]);
        assert!(shell_ids(&testgen::enclosed_scene(1)).is_empty());
        // An empty mesh is neither, and counts for nothing.
        let mut s = testgen::enclosed_scene(3);
        s.meshes.push(crate::scene::Mesh { positions: Vec::new(), normals: None, indices: Vec::new(), hash: None });
        s.objects.push(testgen::object("Nothing", s.meshes.len() as u32 - 1, [1.0; 3], testgen::identity(), None));
        let a = all(&s);
        let c = classify(&s, &a);
        assert_eq!(c.class[a.len() - 1], Class::Empty);
        assert_eq!(content_frame(&a, &c).expect("contents").left_out, [0]);
    }
}
