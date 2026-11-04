//! Templates, as data. Ported from Board's `widget/schema.js`, Copyright (c) 2026 Baden Ballard, MIT
//! (see `THIRD-PARTY-NOTICES.md`).

use super::{refuse_with, s, Facts, Planner, VerbCode, VerbError};
use crate::json::{Fields, Value};
use crate::model::{Id, ObjectType};
use crate::ops::Position;
use std::f64::consts::PI;

/// Every template's name, and its title.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("kickoff", "Project kickoff"),
    ("kanban", "Kanban"),
    ("retro", "Retrospective"),
    ("flowchart", "Flowchart"),
    ("storymap", "User story map"),
    ("brainstorm", "Brainstorm"),
    ("mindmap", "Mind map"),
];

fn n(v: f64) -> Value {
    // The widget keeps one decimal for a position worked out with sines and cosines.
    Value::Num(((v * 10.0) + 0.5).floor() / 10.0)
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

struct T<'p, 'a, 'c> {
    p: &'p mut Planner<'a, 'c>,
    x: f64,
    y: f64,
}

type Done = Result<(), VerbError>;

impl T<'_, '_, '_> {
    fn add(&mut self, kind: ObjectType, f: Fields) -> Result<Id, VerbError> {
        self.p.add_new(&kind, f, &Position::Front)
    }

    fn at(&self, dx: f64, dy: f64, w: f64, h: f64) -> Vec<(&'static str, Value)> {
        vec![
            ("x", n(self.x + dx)),
            ("y", n(self.y + dy)),
            ("w", n(w)),
            ("h", n(h)),
        ]
    }

    fn with(
        &mut self,
        kind: ObjectType,
        (dx, dy, w, h): (f64, f64, f64, f64),
        more: &[(&str, Value)],
    ) -> Result<Id, VerbError> {
        let mut all = fields(&self.at(dx, dy, w, h));
        all.extend(fields(more));
        self.add(kind, all)
    }

    fn frame(&mut self, w: f64, h: f64, title: &str) -> Done {
        self.with(
            ObjectType::Frame,
            (0.0, 0.0, w, h),
            &[("title", s(title)), ("fill", s("white"))],
        )?;
        Ok(())
    }

    fn text(&mut self, dx: f64, dy: f64, w: f64, t: &str, size: f64, bold: bool) -> Done {
        let mut more = vec![("text", s(t)), ("fontSize", Value::Num(size))];
        if bold {
            more.push(("bold", Value::Bool(true)));
        }
        self.with(ObjectType::Text, (dx, dy, w, size * 1.6), &more)?;
        Ok(())
    }

    fn hint(&mut self, dx: f64, dy: f64, w: f64, t: &str) -> Done {
        self.with(
            ObjectType::Text,
            (dx, dy, w, 24.0),
            &[
                ("text", s(t)),
                ("fontSize", Value::Num(14.0)),
                ("color", s("gray")),
            ],
        )?;
        Ok(())
    }

    fn sticky(&mut self, dx: f64, dy: f64, wh: (f64, f64), t: &str, color: &str) -> Done {
        self.with(
            ObjectType::Sticky,
            (dx, dy, wh.0, wh.1),
            &[("text", s(t)), ("color", s(color))],
        )?;
        Ok(())
    }

    fn panel(&mut self, b: (f64, f64, f64, f64), shape: &str, fill: &str) -> Done {
        self.with(
            ObjectType::Shape,
            b,
            &[
                ("shape", s(shape)),
                ("fill", s(fill)),
                ("stroke", s("gray")),
                ("strokeWidth", Value::Num(0.0)),
            ],
        )?;
        Ok(())
    }

    fn shape(
        &mut self,
        b: (f64, f64, f64, f64),
        shape: &str,
        text: &str,
        fill: &str,
    ) -> Result<Id, VerbError> {
        self.with(
            ObjectType::Shape,
            b,
            &[("shape", s(shape)), ("text", s(text)), ("fill", s(fill))],
        )
    }

    fn card(&mut self, b: (f64, f64, f64, f64), pairs: &[(&str, &str)]) -> Done {
        let more: Vec<(&str, Value)> = pairs.iter().map(|(k, v)| (*k, s(v))).collect();
        self.with(ObjectType::Card, b, &more)?;
        Ok(())
    }

    fn link(&mut self, p: &Id, q: &Id, label: &str, ends: (&str, &str), route: &str) -> Done {
        let end = |id: &Id, anchor: &str| {
            Value::Obj(fields(&[
                ("id", Value::Str(id.to_string())),
                ("anchor", s(anchor)),
            ]))
        };
        let mut f = fields(&[
            ("from", end(p, ends.0)),
            ("to", end(q, ends.1)),
            ("route", s(route)),
            ("endCap", s("arrow")),
        ]);
        if !label.is_empty() {
            f.insert("label".into(), s(label));
        }
        self.add(ObjectType::Connector, f)?;
        Ok(())
    }

    fn kanban(&mut self) -> Done {
        let cols = [
            (
                "To do",
                "gray",
                "What\u{2019}s next",
                "Plan the next sprint",
            ),
            (
                "In progress",
                "blue",
                "Being worked on",
                "Interview three customers",
            ),
            ("Review", "violet", "Waiting for a check", "Draft the brief"),
            (
                "Done",
                "green",
                "Finished this week",
                "Kickoff notes shared",
            ),
        ];
        self.frame(4.0 * 320.0 + 40.0, 760.0, "Kanban")?;
        self.text(24.0, 20.0, 600.0, "Kanban board", 32.0, true)?;
        self.hint(26.0, 70.0, 900.0, "Drag cards between columns. Double-click a card to edit it; its Fields button sets owner, due date and status.")?;
        for (i, (name, c, sub, title)) in cols.iter().enumerate() {
            let cx = 24.0 + i as f64 * 320.0;
            self.panel((cx, 110.0, 296.0, 620.0), "round", c)?;
            self.text(cx + 16.0, 122.0, 260.0, name, 20.0, true)?;
            self.hint(cx + 17.0, 156.0, 260.0, sub)?;
            let owner = if i % 2 == 1 { "Teammate" } else { "You" };
            self.card(
                (cx + 8.0, 196.0, 280.0, 140.0),
                &[
                    ("title", title),
                    ("text", "Describe the work"),
                    ("color", c),
                    ("status", name),
                    ("assignee", owner),
                    ("due", ""),
                ],
            )?;
            if i < 2 {
                let title = if i == 1 {
                    "Fix the onboarding copy"
                } else {
                    "Pick a launch date"
                };
                self.card(
                    (cx + 8.0, 352.0, 280.0, 140.0),
                    &[
                        ("title", title),
                        ("text", ""),
                        ("color", c),
                        ("status", name),
                        ("assignee", ""),
                        ("due", ""),
                    ],
                )?;
            }
        }
        Ok(())
    }

    fn retro(&mut self) -> Done {
        let cols = [
            ("Went well", "green", "What should we keep doing?", "green"),
            ("To improve", "pink", "What slowed us down?", "pink"),
            (
                "Action items",
                "blue",
                "What will we change, and who owns it?",
                "cyan",
            ),
        ];
        self.frame(3.0 * 400.0 + 40.0, 800.0, "Retrospective")?;
        self.text(24.0, 20.0, 800.0, "Retrospective", 32.0, true)?;
        self.hint(
            26.0,
            70.0,
            1000.0,
            "Everyone adds notes for 5 minutes, then vote on the ones to act on.",
        )?;
        for (i, (name, c, q, note)) in cols.iter().enumerate() {
            let cx = 24.0 + i as f64 * 400.0;
            self.panel((cx, 110.0, 380.0, 660.0), "round", "gray")?;
            self.with(
                ObjectType::Shape,
                (cx, 110.0, 380.0, 60.0),
                &[
                    ("shape", s("round")),
                    ("fill", s(c)),
                    ("stroke", s(c)),
                    ("strokeWidth", Value::Num(0.0)),
                    ("text", s(name)),
                    ("fontSize", Value::Num(20.0)),
                ],
            )?;
            self.hint(cx + 20.0, 182.0, 340.0, q)?;
            self.sticky(cx + 20.0, 222.0, (160.0, 160.0), "Add a note", note)?;
            self.sticky(cx + 200.0, 222.0, (160.0, 160.0), "", note)?;
        }
        Ok(())
    }

    fn flowchart(&mut self) -> Done {
        self.frame(920.0, 740.0, "Flowchart")?;
        self.text(24.0, 20.0, 600.0, "Flowchart", 32.0, true)?;
        self.hint(26.0, 70.0, 860.0, "Drag from a shape\u{2019}s edge to draw an arrow; arrows stay attached when shapes move.")?;
        let a = self.shape((330.0, 130.0, 180.0, 70.0), "flow-start", "Start", "green")?;
        let b = self.shape(
            (320.0, 260.0, 200.0, 90.0),
            "flow-process",
            "Do the thing",
            "blue",
        )?;
        let c = self.shape(
            (320.0, 410.0, 200.0, 130.0),
            "flow-decision",
            "Worked?",
            "yellow",
        )?;
        let d = self.shape((330.0, 610.0, 180.0, 70.0), "flow-start", "End", "red")?;
        let e = self.shape(
            (620.0, 430.0, 200.0, 90.0),
            "flow-process",
            "Fix it",
            "orange",
        )?;
        self.link(&a, &b, "", ("auto", "auto"), "elbow")?;
        self.link(&b, &c, "", ("auto", "auto"), "elbow")?;
        self.link(&c, &d, "Yes", ("bottom", "top"), "elbow")?;
        self.link(&c, &e, "No", ("right", "left"), "elbow")?;
        self.link(&e, &b, "", ("top", "right"), "elbow")
    }

    fn storymap(&mut self) -> Done {
        let acts = ["Discover", "Sign up", "Use", "Share"];
        let count = acts.len() as f64;
        self.frame(count * 240.0 + 80.0, 820.0, "User story map")?;
        self.text(24.0, 20.0, 600.0, "User story map", 32.0, true)?;
        for (i, name) in acts.iter().enumerate() {
            let cx = 40.0 + i as f64 * 240.0;
            self.sticky(cx, 100.0, (200.0, 110.0), name, "orange")?;
            self.sticky(cx, 240.0, (200.0, 110.0), "Step", "yellow")?;
            self.sticky(cx, 380.0, (200.0, 110.0), "Story (release 1)", "blue")?;
            self.sticky(cx, 600.0, (200.0, 110.0), "Story (release 2)", "violet")?;
        }
        self.panel((24.0, 560.0, count * 240.0 + 32.0, 4.0), "rect", "gray")?;
        self.text(40.0, 520.0, 300.0, "Release 2", 16.0, true)
    }

    fn brainstorm(&mut self) -> Done {
        self.frame(1000.0, 700.0, "Brainstorm")?;
        self.with(
            ObjectType::Shape,
            (380.0, 280.0, 240.0, 140.0),
            &[
                ("shape", s("ellipse")),
                ("text", s("Big question?")),
                ("fill", s("violet")),
                ("fontSize", Value::Num(22.0)),
            ],
        )?;
        let colours = ["yellow", "pink", "blue", "green", "orange", "cyan"];
        for (i, colour) in colours.iter().enumerate() {
            let a = (i as f64 / 6.0) * PI * 2.0 - PI / 2.0;
            let (dx, dy) = (420.0 + a.cos() * 340.0, 270.0 + a.sin() * 230.0);
            self.sticky(dx, dy, (160.0, 160.0), "", colour)?;
        }
        Ok(())
    }

    fn mindmap(&mut self) -> Done {
        self.frame(1000.0, 580.0, "Mind map")?;
        let root = self.with(
            ObjectType::Mind,
            (400.0, 260.0, 200.0, 60.0),
            &[("text", s("Central idea")), ("color", s("violet"))],
        )?;
        for (t, sx, sy) in [
            ("Goals", -1.0, -1.0),
            ("People", 1.0, -1.0),
            ("Risks", -1.0, 1.0),
            ("Timeline", 1.0, 1.0),
        ] {
            let colour = if sx < 0.0 { "blue" } else { "green" };
            self.with(
                ObjectType::Mind,
                (400.0 + sx * 300.0, 260.0 + sy * 110.0, 160.0, 48.0),
                &[
                    ("text", s(t)),
                    ("parent", Value::Str(root.to_string())),
                    ("color", s(colour)),
                ],
            )?;
        }
        Ok(())
    }

    fn section(&mut self, b: (f64, f64, f64, f64), title: &str) -> Done {
        self.panel(b, "round", "gray")?;
        self.text(b.0 + 18.0, b.1 + 12.0, b.2 - 36.0, title, 20.0, true)
    }

    fn kickoff(&mut self) -> Done {
        self.frame(1560.0, 1020.0, "Project kickoff")?;
        self.text(24.0, 20.0, 900.0, "Project kickoff", 32.0, true)?;
        self.hint(26.0, 70.0, 1200.0, "Fill each part together in the first meeting; the agents with access can read it and help.")?;
        self.section((24.0, 110.0, 480.0, 300.0), "Goals")?;
        for (i, (t, c)) in [
            ("What does success look like?", "yellow"),
            ("Who is it for?", "green"),
            ("What is out of scope?", "pink"),
        ]
        .iter()
        .enumerate()
        {
            self.sticky(40.0 + i as f64 * 154.0, 160.0, (142.0, 142.0), t, c)?;
        }
        self.section((528.0, 110.0, 500.0, 300.0), "Team and roles")?;
        for (i, (r, d, c)) in [
            ("Owner", "Decides and unblocks", "violet"),
            ("Builder", "Does the work", "blue"),
            ("Reviewer", "Checks before it ships", "green"),
        ]
        .iter()
        .enumerate()
        {
            self.card(
                (544.0, 156.0 + i as f64 * 80.0, 468.0, 70.0),
                &[
                    ("title", r),
                    ("text", d),
                    ("color", c),
                    ("assignee", ""),
                    ("status", ""),
                ],
            )?;
        }
        self.section((1052.0, 110.0, 484.0, 300.0), "Risks")?;
        for (i, t) in ["What could go wrong?", "What do we depend on?"]
            .iter()
            .enumerate()
        {
            self.sticky(
                1068.0 + i as f64 * 230.0,
                160.0,
                (210.0, 140.0),
                t,
                "orange",
            )?;
        }
        self.section((24.0, 434.0, 1512.0, 220.0), "Milestones")?;
        let names = [
            "Kickoff",
            "Plan agreed",
            "First version",
            "Review",
            "Launch",
        ];
        let mut shapes = Vec::new();
        for (i, t) in names.iter().enumerate() {
            let last = i == names.len() - 1;
            shapes.push(self.with(
                ObjectType::Shape,
                (56.0 + i as f64 * 300.0, 500.0, 220.0, 110.0),
                &[
                    ("shape", s(if last { "star" } else { "round" })),
                    ("text", s(t)),
                    ("fill", s(if last { "yellow" } else { "white" })),
                    ("fontSize", Value::Num(18.0)),
                ],
            )?);
        }
        for pair in shapes.windows(2) {
            self.link(&pair[0], &pair[1], "", ("right", "left"), "straight")?;
        }
        self.section((24.0, 678.0, 600.0, 316.0), "Open questions")?;
        self.sticky(
            40.0,
            728.0,
            (180.0, 140.0),
            "What do we still need to decide?",
            "violet",
        )?;
        self.section((648.0, 678.0, 888.0, 316.0), "First steps")?;
        for (i, (t, c, title)) in [
            ("To do", "gray", "Book the first review"),
            ("Doing", "blue", "Write the plan"),
            ("Done", "green", "Kickoff held"),
        ]
        .iter()
        .enumerate()
        {
            let i = i as f64;
            self.text(668.0 + i * 290.0, 722.0, 200.0, t, 16.0, true)?;
            self.card(
                (664.0 + i * 290.0, 756.0, 270.0, 110.0),
                &[
                    ("title", title),
                    ("text", ""),
                    ("color", c),
                    ("status", t),
                    ("assignee", ""),
                ],
            )?;
        }
        Ok(())
    }
}

impl Planner<'_, '_> {
    pub(crate) fn template(&mut self, name: &str, x: f64, y: f64) -> Result<(), VerbError> {
        let mut t = T { p: self, x, y };
        match name {
            "kickoff" => t.kickoff(),
            "kanban" => t.kanban(),
            "retro" => t.retro(),
            "flowchart" => t.flowchart(),
            "storymap" => t.storymap(),
            "brainstorm" => t.brainstorm(),
            "mindmap" => t.mindmap(),
            _ => Err(refuse_with(
                VerbCode::NoTemplate,
                format!("no template {name:?}"),
                Facts {
                    name: Some(name.to_string()),
                    ..Facts::default()
                },
            )),
        }
    }
}
