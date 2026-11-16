//! The scene's watched turn, pinned whole as a golden (`tests/watch_turn_golden/scene_turn.json`):
//! what a person watching a live scene receives while an agent arranges a held set, made only by
//! the pack's own verbs on a generated scene and the kit's own frames, so an app's sample turn is
//! never written by hand and real frames replace it as they are.
//!
//! **The scene** ([`room`]): a camera and a lamp in the scene, and a collection `Set` holding
//! `Table`, `Vase` (parented to `Table`) and `Chair`.
//!
//! **The turn,** one step at a time:
//!
//! 1. `watch`: the person (`ada`) opens a keyed watch and takes its snapshot.
//! 2. `claim`: the agent (`atlas`) holds `Table`, `Vase` and `Chair` (a claim with `hold`).
//! 3. `place Table`: it moves and turns `Table` (`object place`, its turn, the host's claim
//!    policy that respects claims); `Vase` goes with it, as its child.
//! 4. `place Chair`: it moves and turns `Chair`.
//! 5. `refused`: a second agent (`bolt`) tries to move `Vase`; the claim refuses it.
//! 6. `release`: the agent lets the set go.
//!
//! **What a step records:** who acted, the verb's words and exit, every keyed-watch frame the
//! person's watch received (`WatchFrame::to_value`, the kit's own JSON, as a host forwards it),
//! and every roster event the person's roster subscription received, encoded as the wire carries
//! it (`k2-mpc-base`'s `client/WIRE.md`): the kit hands a host the events, not their JSON, so
//! [`roster_event`] mirrors the kit's own pinned encoder (its `tests/wire_golden.rs`) for the
//! kinds a turn yields; any other kind fails the row, so a new one is seen.
//!
//! **Only what is random per run is rewritten:** each connection's client ID becomes a fixed
//! number (`ada` 1, `atlas` 2, `bolt` 3) and each claim's id `claim-N` in order of appearance,
//! wherever they appear. The host's clock is fixed and moved by fixed amounts.
//!
//! **Re-bless** on purpose only: `MPC_BLESS_GOLDEN=1` (`tests/kit_view_golden/mod.rs`).

#[path = "kit_view_golden/mod.rs"]
#[allow(dead_code)]
mod golden;
mod live_support;

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use k2_mpc_base::elem::Key;
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::session::{
    ClaimActor, ClaimPolicy, ConnId, ElemFocus, ElemSelection, KeyedWatch, LineRange, Participant,
    ParticipantKind, Rights, Role, RosterEvent, Session, Stay, WatchFrame, WatchOptions, Who,
    KEYED_WATCH_FEATURE,
};
use k2_mpc_base::tools::ToolOptions;
use k2_mpc_base::turn::Turn;
use k2_mpc_blend::live::OBJS;
use k2_mpc_blend::testgen::{ObjectSpec, SceneBuilder};
use live_support::{content, fx, obj_named};

/// The golden's name, under `tests/watch_turn_golden/`.
const GOLDEN: &str = "scene_turn";

/// The scene: a camera and a lamp in the scene; `Set` holds `Table`, `Vase` (parented to
/// `Table`) and `Chair`. Every object rotates by Euler angles.
fn room() -> Vec<u8> {
    let mut b = SceneBuilder::placed(403);
    b.ui();
    b.scene("Scene", &["Camera", "Lamp"], &["Set"], Some("Camera"));
    b.collection("Set", &["Table", "Vase", "Chair"], &[]);
    let obj = |ty: i16, data: &str, loc: [f32; 3]| ObjectSpec {
        ty,
        data: Some(data.to_owned()),
        loc,
        rotmode: 1,
        ..ObjectSpec::default()
    };
    b.object(
        "Camera",
        &ObjectSpec {
            rot: [1.1, 0.0, 0.8],
            ..obj(11, "CA:Cam", [7.0, -7.0, 5.0])
        },
    );
    b.object("Lamp", &obj(10, "LA:Lamp", [4.0, -1.0, 6.0]));
    b.object("Table", &obj(1, "ME:Table", [0.0, 0.0, 0.0]));
    b.object(
        "Vase",
        &ObjectSpec {
            parent: Some("Table".into()),
            ..obj(1, "ME:Vase", [0.25, 0.0, 0.75])
        },
    );
    b.object("Chair", &obj(1, "ME:Chair", [1.25, 0.0, 0.0]));
    b.camera("Cam", 50.0);
    b.light("Lamp", 10.0);
    b.mesh("Table", &[], 8);
    b.mesh("Vase", &[], 8);
    b.mesh("Chair", &[], 8);
    b.bytes()
}

// ---------- the wire's roster encodings (as the kit's `tests/wire_golden.rs`) ----------

/// Fixed numbers for the run's client IDs, and claim ids in order of appearance.
#[derive(Default)]
struct Stable {
    clients: HashMap<u64, u64>,
    claims: HashMap<String, String>,
}

impl Stable {
    fn client(&self, c: u64) -> u64 {
        *self
            .clients
            .get(&c)
            .unwrap_or_else(|| panic!("client {c} isn't one of the turn's"))
    }
    fn claim(&mut self, id: &str) -> String {
        let n = self.claims.len() + 1;
        self.claims
            .entry(id.to_owned())
            .or_insert_with(|| format!("claim-{n}"))
            .clone()
    }
    /// A keyed-watch frame with its client IDs and claim ids rewritten, nothing else.
    fn frame(&mut self, v: &Json) -> Json {
        match v {
            Json::Obj(m) => Json::Obj(
                m.iter()
                    .map(|(k, x)| {
                        let x = match (k.as_str(), x.as_u64(), x) {
                            ("client", Some(c), _) => Json::from(self.client(c)),
                            ("claim", _, Json::Str(c)) => Json::from(self.claim(c)),
                            _ => self.frame(x),
                        };
                        (k.clone(), x)
                    })
                    .collect(),
            ),
            Json::List(l) => Json::List(l.iter().map(|x| self.frame(x)).collect()),
            other => other.clone(),
        }
    }
}

fn who(w: &Who) -> Json {
    Json::from(w.to_string())
}

fn kind(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Person => "person",
        ParticipantKind::Agent => "agent",
        ParticipantKind::App => "app",
        ParticipantKind::External => "external",
        ParticipantKind::Restore => "restore",
        ParticipantKind::Import => "import",
    }
}

fn role(r: Role) -> &'static str {
    match r {
        Role::Editor => "editor",
        Role::Viewer => "viewer",
    }
}

fn rights(r: Rights) -> &'static str {
    if r.read && r.edit {
        "edit"
    } else if r.read {
        "read"
    } else {
        "none"
    }
}

fn lines(l: &LineRange) -> Json {
    Json::obj([
        ("root", Json::from(l.root.as_str())),
        ("first", Json::from(l.first)),
        ("last", Json::from(l.last)),
    ])
}

fn actor(w: &Who, a: &ClaimActor) -> Json {
    Json::obj([
        ("id", who(w)),
        ("name", Json::from(a.name.as_str())),
        ("kind", Json::from(kind(a.kind))),
        ("colour", Json::from(a.colour.as_str())),
    ])
}

fn addresses_value(addresses: &[String]) -> Json {
    Json::List(addresses.iter().map(|a| Json::from(a.as_str())).collect())
}

fn selection(s: &ElemSelection) -> Json {
    Json::obj([
        ("root", Json::from(s.root.as_str())),
        ("keys", addresses_value(&s.addresses)),
    ])
}

fn focus(f: &ElemFocus) -> Json {
    Json::obj([
        ("root", Json::from(f.root.as_str())),
        ("key", Json::from(f.address.as_str())),
    ])
}

/// A `selection`, `focus` or `view_address` event: `client` only when the event names one.
fn elem_event(kind: &'static str, w: &Who, value: Json, client: &Option<u64>, st: &Stable) -> Json {
    let mut fields = vec![("kind", Json::from(kind)), ("who", who(w)), (kind, value)];
    if let Some(c) = client {
        fields.push(("client", Json::from(st.client(*c))));
    }
    Json::obj(fields)
}

/// One roster event as the wire carries it, for the kinds a turn yields.
fn roster_event(e: &RosterEvent, st: &mut Stable) -> Json {
    match e {
        RosterEvent::Selection {
            who: w,
            selection: s,
            client,
            ..
        } => elem_event(
            "selection",
            w,
            s.as_ref().map_or(Json::Null, selection),
            client,
            st,
        ),
        RosterEvent::Focus {
            who: w,
            focus: f,
            client,
            ..
        } => elem_event("focus", w, f.as_ref().map_or(Json::Null, focus), client, st),
        RosterEvent::ViewAddress {
            who: w,
            address,
            client,
            ..
        } => elem_event(
            "view_address",
            w,
            address.as_deref().map_or(Json::Null, Json::from),
            client,
            st,
        ),
        RosterEvent::ClaimStartedElems {
            who: w,
            claim,
            root,
            addresses,
            note,
            hold,
            expires_ms,
            version,
            actor: a,
            started_ms,
            ..
        } => Json::obj([
            ("kind", Json::from("claim_started_elems")),
            ("who", who(w)),
            ("claim", Json::from(st.claim(&claim.to_string()))),
            ("root", Json::from(root.as_str())),
            ("keys", addresses_value(addresses)),
            ("note", note.as_deref().map_or(Json::Null, Json::from)),
            ("hold", Json::from(*hold)),
            ("expires_ms", Json::from(*expires_ms)),
            ("version", Json::from(*version)),
            ("actor", actor(w, a)),
            ("started_ms", Json::from(*started_ms)),
        ]),
        RosterEvent::ClaimEnded {
            who: w,
            claim,
            how,
            lines: l,
            version,
            actor: a,
            at_ms,
            at_rev,
            addresses,
            ..
        } => {
            let mut v = vec![
                ("kind", Json::from("claim_ended")),
                ("who", who(w)),
                ("claim", Json::from(st.claim(&claim.to_string()))),
                ("how", Json::from(how.as_str())),
                ("lines", lines(l)),
                ("version", Json::from(*version)),
                ("actor", actor(w, a)),
                ("at_ms", Json::from(*at_ms)),
                ("at_rev", at_rev.map_or(Json::Null, Json::from)),
            ];
            if !addresses.is_empty() {
                v.push(("keys", addresses_value(addresses)));
            }
            Json::obj(v)
        }
        RosterEvent::Note {
            to,
            from,
            kind: k,
            lines: l,
            claim,
            ..
        } => Json::obj([
            ("kind", Json::from("note")),
            ("to", who(to)),
            ("from", who(from)),
            ("note", Json::from(k.as_str())),
            ("lines", lines(l)),
            (
                "claim",
                claim.map_or(Json::Null, |c| Json::from(st.claim(&c.to_string()))),
            ),
        ]),
        RosterEvent::Role {
            who: w, role: r, ..
        } => Json::obj([
            ("kind", Json::from("role")),
            ("who", who(w)),
            ("role", Json::from(role(*r))),
        ]),
        RosterEvent::Rights {
            who: w, rights: r, ..
        } => Json::obj([
            ("kind", Json::from("rights")),
            ("who", who(w)),
            ("rights", Json::from(rights(*r))),
        ]),
        RosterEvent::Away { who: w, away, .. } => Json::obj([
            ("kind", Json::from("away")),
            ("who", who(w)),
            ("away", Json::from(*away)),
        ]),
        RosterEvent::Watching { count, .. } => Json::obj([
            ("kind", Json::from("watching")),
            ("count", Json::from(*count)),
        ]),
        other => panic!("a roster event this golden doesn't encode yet: {other:?}"),
    }
}

// ---------- the turn ----------

/// The participants, their connections, and the person's watch and roster. The fixture is
/// last, so the session goes before its folder does.
struct Turnout {
    s: std::sync::Arc<Session>,
    ada: ConnId,
    atlas: ConnId,
    bolt: ConnId,
    watch: KeyedWatch,
    roster: Receiver<RosterEvent>,
    st: Stable,
    steps: Vec<Json>,
    f: live_support::Fx,
}

impl Turnout {
    fn open(tag: &str) -> Self {
        let f = fx(tag, |_| {});
        f.write(&room());
        let s = f.open();
        let mut st = Stable::default();
        let mut join = |pid: &str, kind: ParticipantKind, colour: &str, n: u64| {
            let j = s
                .join(Participant::new(pid, pid, colour, kind, "cli"), Stay::Held)
                .expect("setup: the join");
            st.clients.insert(j.client, n);
            j.conn
        };
        let ada = join("ada", ParticipantKind::Person, "#2f7de1", 1);
        let atlas = join("atlas", ParticipantKind::Agent, "#d9480f", 2);
        let bolt = join("bolt", ParticipantKind::Agent, "#2b8a3e", 3);
        let roster = s.subscribe_roster(ada).expect("setup: the roster");
        s.flush_roster().expect("setup: the roster's flush");
        // the subscription's own snapshot and the joins are before the turn
        while roster.try_recv().is_ok() {}
        let watch = s
            .watch_keyed(ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .expect("setup: the watch")
            .expect("setup: the host lists the keyed watch");
        Turnout {
            s,
            ada,
            atlas,
            bolt,
            watch,
            roster,
            st,
            steps: Vec::new(),
            f,
        }
    }

    /// The live object named `name`: its key and address.
    fn object(&self, name: &str) -> (Key, String) {
        let k = obj_named(&content(&self.s, self.ada), name)
            .unwrap_or_else(|| panic!("setup: no live object {name}"));
        let address = format!("@obj:{}", &k[1..]);
        (Key::new(k).expect("setup: a live key"), address)
    }

    /// Records one step: the frames `first` (a snapshot's), then what the person's watch and
    /// roster received since the last step.
    fn step_with(
        &mut self,
        first: Vec<WatchFrame>,
        name: &str,
        by: &str,
        argv: Option<&[String]>,
        exit: Option<u8>,
    ) {
        self.f.advance(250);
        self.s.flush_roster().expect("the roster's flush");
        let mut frames = first;
        frames.extend(self.watch.try_frames());
        let watch: Vec<Json> = frames
            .iter()
            .map(|fr| self.st.frame(&fr.to_value()))
            .collect();
        let events: Vec<RosterEvent> = self.roster.try_iter().collect();
        let roster: Vec<Json> = events
            .iter()
            .map(|e| roster_event(e, &mut self.st))
            .collect();
        self.steps.push(Json::obj([
            ("step", Json::from(name)),
            ("by", Json::from(by)),
            (
                "argv",
                argv.map_or(Json::Null, |a| {
                    Json::List(a.iter().map(|s| Json::from(s.as_str())).collect())
                }),
            ),
            (
                "exit",
                exit.map_or(Json::Null, |e| Json::from(u64::from(e))),
            ),
            ("watch", Json::List(watch)),
            ("roster", Json::List(roster)),
        ]));
    }

    /// Records one step with no frames of its own.
    fn step(&mut self, name: &str, by: &str, argv: Option<&[String]>, exit: Option<u8>) {
        self.step_with(Vec::new(), name, by, argv, exit);
    }

    /// An agent's edit verb at the version of its own read just before, as a host runs it:
    /// with its turn, under the policy that respects others' claims. Its exit, recorded.
    fn verb(&mut self, name: &str, by: &str, c: ConnId, turn: &str, words: &[&str]) {
        let read = live_support::args(&["scene", "list", "--limit", "1"]);
        let base = self
            .s
            .run_tool(c, OBJS, &read, b"")
            .expect("the agent's read")
            .version;
        let mut argv = live_support::args(words);
        argv.push("--base".into());
        argv.push(base.to_string());
        let mut opts = ToolOptions::new();
        opts.policy = ClaimPolicy::Respect;
        opts.turn = Some(Turn::new(turn).expect("a turn id"));
        let exit = match self.s.run_tool_with(c, OBJS, &argv, b"", &opts) {
            Ok(o) => o.exit,
            Err(e) => e.exit(),
        };
        self.step(name, by, Some(argv.as_slice()), Some(exit));
    }
}

/// The turn, run once in a fresh fixture: the golden's JSON.
fn turn(tag: &str) -> Json {
    let mut t = Turnout::open(tag);
    let snapshot =
        t.s.watch_snapshot(&mut t.watch)
            .expect("the watch's snapshot");
    t.step_with(snapshot, "watch", "ada", None, None);
    let (table, table_at) = t.object("Table");
    let (vase, vase_at) = t.object("Vase");
    let (chair, chair_at) = t.object("Chair");
    let claim =
        t.s.claim_elems(
            t.atlas,
            OBJS,
            &[table, vase, chair],
            None,
            Some("arranging the set".into()),
            true,
        )
        .expect("the agent's claim");
    t.step("claim", "atlas", None, None);
    let atlas = t.atlas;
    t.verb(
        "place Table",
        "atlas",
        atlas,
        "turn-1",
        &[
            "object",
            "place",
            table_at.as_str(),
            "--loc",
            "0.5,0.25,0",
            "--rot",
            "0,0,0.5",
        ],
    );
    t.verb(
        "place Chair",
        "atlas",
        atlas,
        "turn-1",
        &[
            "object",
            "place",
            chair_at.as_str(),
            "--loc",
            "1.5,-0.75,0",
            "--rot",
            "0,0,-0.5",
        ],
    );
    let bolt = t.bolt;
    t.verb(
        "refused",
        "bolt",
        bolt,
        "turn-2",
        &["object", "place", vase_at.as_str(), "--loc", "0,0,2"],
    );
    t.s.release_claim(atlas, Some(claim.id))
        .expect("the agent lets go");
    t.step("release", "atlas", None, None);
    Json::obj([
        (
            "scene",
            Json::from("a camera and a lamp; Set holds Table, Vase (parented to Table) and Chair"),
        ),
        ("steps", Json::List(t.steps)),
    ])
}

/// Each step's frame kinds (`type`) and roster kinds (`kind`), in order.
fn kinds(v: &Json, part: &str, field: &str) -> Vec<String> {
    let Some(Json::List(steps)) = v.get("steps") else {
        return Vec::new();
    };
    steps
        .iter()
        .filter_map(|s| match s.get(part) {
            Some(Json::List(l)) => Some(l),
            _ => None,
        })
        .flatten()
        .filter_map(|x| x.get(field).and_then(Json::as_str).map(str::to_owned))
        .collect()
}

fn count(all: &[String], k: &str) -> usize {
    all.iter().filter(|x| x.as_str() == k).count()
}

#[test]
fn the_scene_turn_is_the_golden() {
    let got = turn("turn-golden");
    // what the turn exercised, before it is compared: an empty turn never passes
    let frames = kinds(&got, "watch", "type");
    let events = kinds(&got, "roster", "kind");
    let n = |k: &str| count(&frames, k) + count(&events, k);
    for (k, want) in [
        ("keyed_snapshot", 1),
        ("ops_change", 2),
        ("verb_refused", 1),
        ("claim_started_elems", 1),
        ("claim_ended", 1),
    ] {
        assert_eq!(
            n(k),
            want,
            "the turn's {k} frames or events: {frames:?} {events:?}"
        );
    }
    println!(
        "turn golden: {} steps, {} watch frames, {} roster events",
        match got.get("steps") {
            Some(Json::List(l)) => l.len(),
            _ => 0,
        },
        frames.len(),
        events.len()
    );
    if let Err(e) = golden::check_in("watch_turn_golden", GOLDEN, &got) {
        panic!("{e}");
    }
}

#[test]
fn the_scene_turn_is_reproducible() {
    let a = turn("turn-again-a").to_json_pretty();
    let b = turn("turn-again-b").to_json_pretty();
    assert!(a.len() > 1_000, "the turn is {} bytes of JSON", a.len());
    assert!(a == b, "two runs of the turn differ");
    println!("turn golden: reproducible, {} bytes twice", a.len());
}
