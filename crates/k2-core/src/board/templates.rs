//! Board templates: each makes the objects of one starter layout at an
//! origin (x, y), ready to add (the widget's `schema.js` templates, the
//! same layouts).

use serde_json::{json, Map, Value};

use super::schema::new_id;

/// Template names and their labels, in the order the widget lists them.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("kickoff", "Project kickoff"),
    ("kanban", "Kanban"),
    ("retro", "Retrospective"),
    ("flowchart", "Flowchart"),
    ("storymap", "User story map"),
    ("brainstorm", "Brainstorm"),
    ("mindmap", "Mind map"),
];

struct T {
    out: Vec<Value>,
}

impl T {
    fn add(&mut self, ty: &str, f: Value) -> String {
        let id = new_id(ty);
        let mut o = Map::new();
        o.insert("id".into(), Value::String(id.clone()));
        o.insert("type".into(), Value::String(ty.into()));
        if let Value::Object(m) = f {
            o.extend(m);
        }
        self.out.push(Value::Object(o));
        id
    }
    fn frame(&mut self, x: f64, y: f64, w: f64, h: f64, title: &str) -> String {
        self.add("frame", json!({ "x": x, "y": y, "w": w, "h": h, "title": title, "fill": "white" }))
    }
    fn sticky(&mut self, x: f64, y: f64, text: &str, color: &str) -> String {
        self.add("sticky", json!({ "x": x, "y": y, "w": 160, "h": 160, "text": text, "color": color }))
    }
    fn text(&mut self, x: f64, y: f64, w: f64, t: &str, size: f64, bold: bool) -> String {
        self.add("text", json!({ "x": x, "y": y, "w": w, "h": size * 1.6, "text": t, "fontSize": size, "bold": bold }))
    }
    fn hint(&mut self, x: f64, y: f64, w: f64, t: &str) -> String {
        self.add("text", json!({ "x": x, "y": y, "w": w, "h": 24, "text": t, "fontSize": 14, "color": "gray" }))
    }
    fn link(&mut self, p: &str, q: &str, label: &str, fa: &str, ta: &str, route: &str) -> String {
        self.add(
            "connector",
            json!({ "from": { "id": p, "anchor": fa }, "to": { "id": q, "anchor": ta }, "route": route, "endCap": "arrow", "label": label }),
        )
    }
    fn section(&mut self, x: f64, y: f64, w: f64, h: f64, title: &str) {
        self.add("shape", json!({ "x": x, "y": y, "w": w, "h": h, "shape": "round", "fill": "gray", "stroke": "gray", "strokeWidth": 0 }));
        self.text(x + 18.0, y + 12.0, w - 36.0, title, 20.0, true);
    }
}

/// The objects of template `name` at (x, y), or `None` for an unknown name.
pub fn build(name: &str, x: f64, y: f64) -> Option<Vec<Value>> {
    let mut t = T { out: Vec::new() };
    match name {
        "kanban" => {
            let cols = [
                ("To do", "gray", "What’s next"),
                ("In progress", "blue", "Being worked on"),
                ("Review", "violet", "Waiting for a check"),
                ("Done", "green", "Finished this week"),
            ];
            t.frame(x, y, 4.0 * 320.0 + 40.0, 760.0, "Kanban");
            t.text(x + 24.0, y + 20.0, 600.0, "Kanban board", 32.0, true);
            t.hint(x + 26.0, y + 70.0, 900.0, "Drag cards between columns. Double-click a card to edit it; its Fields button sets owner, due date and status.");
            for (i, (name, c, sub)) in cols.iter().enumerate() {
                let cx = x + 24.0 + i as f64 * 320.0;
                t.add("shape", json!({ "x": cx, "y": y + 110.0, "w": 296, "h": 620, "shape": "round", "fill": c, "stroke": "gray", "strokeWidth": 0, "text": "" }));
                t.text(cx + 16.0, y + 122.0, 260.0, name, 20.0, true);
                t.hint(cx + 17.0, y + 156.0, 260.0, sub);
                let title = match *name {
                    "Done" => "Kickoff notes shared",
                    "Review" => "Draft the brief",
                    "In progress" => "Interview three customers",
                    _ => "Plan the next sprint",
                };
                t.add("card", json!({ "x": cx + 8.0, "y": y + 196.0, "w": 280, "h": 140, "title": title, "text": "Describe the work", "color": c, "status": name, "assignee": if i % 2 == 1 { "Teammate" } else { "You" }, "due": "" }));
                if i < 2 {
                    let title = if i == 1 { "Fix the onboarding copy" } else { "Pick a launch date" };
                    t.add("card", json!({ "x": cx + 8.0, "y": y + 352.0, "w": 280, "h": 140, "title": title, "text": "", "color": c, "status": name, "assignee": "", "due": "" }));
                }
            }
        }
        "retro" => {
            let cols = [
                ("Went well", "green", "What should we keep doing?"),
                ("To improve", "pink", "What slowed us down?"),
                ("Action items", "blue", "What will we change, and who owns it?"),
            ];
            t.frame(x, y, 3.0 * 400.0 + 40.0, 800.0, "Retrospective");
            t.text(x + 24.0, y + 20.0, 800.0, "Retrospective", 32.0, true);
            t.hint(x + 26.0, y + 70.0, 1000.0, "Everyone adds notes for 5 minutes (the timer is top right), then vote on the ones to act on.");
            for (i, (name, c, q)) in cols.iter().enumerate() {
                let cx = x + 24.0 + i as f64 * 400.0;
                let note = if *c == "blue" { "cyan" } else { c };
                t.add("shape", json!({ "x": cx, "y": y + 110.0, "w": 380, "h": 660, "shape": "round", "fill": "gray", "stroke": "gray", "strokeWidth": 0 }));
                t.add("shape", json!({ "x": cx, "y": y + 110.0, "w": 380, "h": 60, "shape": "round", "fill": c, "stroke": c, "strokeWidth": 0, "text": name, "fontSize": 20 }));
                t.hint(cx + 20.0, y + 182.0, 340.0, q);
                t.sticky(cx + 20.0, y + 222.0, "Add a note", note);
                t.sticky(cx + 200.0, y + 222.0, "", note);
            }
        }
        "flowchart" => {
            t.frame(x, y, 920.0, 740.0, "Flowchart");
            t.text(x + 24.0, y + 20.0, 600.0, "Flowchart", 32.0, true);
            t.hint(x + 26.0, y + 70.0, 860.0, "Drag from a shape’s edge to draw an arrow; arrows stay attached when shapes move.");
            let a = t.add("shape", json!({ "x": x + 330.0, "y": y + 130.0, "w": 180, "h": 70, "shape": "flow-start", "text": "Start", "fill": "green" }));
            let b = t.add("shape", json!({ "x": x + 320.0, "y": y + 260.0, "w": 200, "h": 90, "shape": "flow-process", "text": "Do the thing", "fill": "blue" }));
            let c = t.add("shape", json!({ "x": x + 320.0, "y": y + 410.0, "w": 200, "h": 130, "shape": "flow-decision", "text": "Worked?", "fill": "yellow" }));
            let d = t.add("shape", json!({ "x": x + 330.0, "y": y + 610.0, "w": 180, "h": 70, "shape": "flow-start", "text": "End", "fill": "red" }));
            let e = t.add("shape", json!({ "x": x + 620.0, "y": y + 430.0, "w": 200, "h": 90, "shape": "flow-process", "text": "Fix it", "fill": "orange" }));
            t.link(&a, &b, "", "auto", "auto", "elbow");
            t.link(&b, &c, "", "auto", "auto", "elbow");
            t.link(&c, &d, "Yes", "bottom", "top", "elbow");
            t.link(&c, &e, "No", "right", "left", "elbow");
            t.link(&e, &b, "", "top", "right", "elbow");
        }
        "storymap" => {
            let acts = ["Discover", "Sign up", "Use", "Share"];
            let n = acts.len() as f64;
            t.frame(x, y, n * 240.0 + 80.0, 820.0, "User story map");
            t.text(x + 24.0, y + 20.0, 600.0, "User story map", 32.0, true);
            for (i, name) in acts.iter().enumerate() {
                let cx = x + 40.0 + i as f64 * 240.0;
                t.add("sticky", json!({ "x": cx, "y": y + 100.0, "w": 200, "h": 110, "text": name, "color": "orange" }));
                t.add("sticky", json!({ "x": cx, "y": y + 240.0, "w": 200, "h": 110, "text": "Step", "color": "yellow" }));
                t.add("sticky", json!({ "x": cx, "y": y + 380.0, "w": 200, "h": 110, "text": "Story (release 1)", "color": "blue" }));
                t.add("sticky", json!({ "x": cx, "y": y + 600.0, "w": 200, "h": 110, "text": "Story (release 2)", "color": "violet" }));
            }
            t.add("shape", json!({ "x": x + 24.0, "y": y + 560.0, "w": n * 240.0 + 32.0, "h": 4, "shape": "rect", "fill": "gray", "stroke": "gray", "strokeWidth": 0 }));
            t.text(x + 40.0, y + 520.0, 300.0, "Release 2", 16.0, true);
        }
        "brainstorm" => {
            t.frame(x, y, 1000.0, 700.0, "Brainstorm");
            t.add("shape", json!({ "x": x + 380.0, "y": y + 280.0, "w": 240, "h": 140, "shape": "ellipse", "text": "Big question?", "fill": "violet", "fontSize": 22 }));
            let colors = ["yellow", "pink", "blue", "green", "orange", "cyan"];
            for (i, c) in colors.iter().enumerate() {
                let a = (i as f64 / 6.0) * std::f64::consts::PI * 2.0 - std::f64::consts::PI / 2.0;
                t.sticky(x + 420.0 + a.cos() * 340.0, y + 270.0 + a.sin() * 230.0, "", c);
            }
        }
        "mindmap" => {
            t.frame(x, y, 1000.0, 580.0, "Mind map");
            let root = t.add("mind", json!({ "x": x + 400.0, "y": y + 260.0, "w": 200, "h": 60, "text": "Central idea", "color": "violet" }));
            for (label, sx, sy) in [("Goals", -1.0, -1.0), ("People", 1.0, -1.0), ("Risks", -1.0, 1.0), ("Timeline", 1.0, 1.0)] {
                t.add(
                    "mind",
                    json!({ "x": x + 400.0 + sx * 300.0, "y": y + 260.0 + sy * 110.0, "w": 160, "h": 48, "text": label, "parent": root, "color": if sx < 0.0 { "blue" } else { "green" } }),
                );
            }
        }
        "kickoff" => {
            t.frame(x, y, 1560.0, 1020.0, "Project kickoff");
            t.text(x + 24.0, y + 20.0, 900.0, "Project kickoff", 32.0, true);
            t.hint(x + 26.0, y + 70.0, 1200.0, "Fill each part together in the first meeting; the agents with access can read it and help.");
            t.section(x + 24.0, y + 110.0, 480.0, 300.0, "Goals");
            for (i, (text, c)) in [("What does success look like?", "yellow"), ("Who is it for?", "green"), ("What is out of scope?", "pink")].iter().enumerate() {
                t.add("sticky", json!({ "x": x + 40.0 + i as f64 * 154.0, "y": y + 160.0, "w": 142, "h": 142, "text": text, "color": c }));
            }
            t.section(x + 528.0, y + 110.0, 500.0, 300.0, "Team and roles");
            for (i, (r, d, c)) in [("Owner", "Decides and unblocks", "violet"), ("Builder", "Does the work", "blue"), ("Reviewer", "Checks before it ships", "green")].iter().enumerate() {
                t.add("card", json!({ "x": x + 544.0, "y": y + 156.0 + i as f64 * 80.0, "w": 468, "h": 70, "title": r, "text": d, "color": c, "assignee": "", "status": "" }));
            }
            t.section(x + 1052.0, y + 110.0, 484.0, 300.0, "Risks");
            for (i, text) in ["What could go wrong?", "What do we depend on?"].iter().enumerate() {
                t.add("sticky", json!({ "x": x + 1068.0 + i as f64 * 230.0, "y": y + 160.0, "w": 210, "h": 140, "text": text, "color": "orange" }));
            }
            t.section(x + 24.0, y + 434.0, 1512.0, 220.0, "Milestones");
            let ms = ["Kickoff", "Plan agreed", "First version", "Review", "Launch"];
            let mut shapes = Vec::new();
            for (i, m) in ms.iter().enumerate() {
                let last = i == ms.len() - 1;
                shapes.push(t.add(
                    "shape",
                    json!({ "x": x + 56.0 + i as f64 * 300.0, "y": y + 500.0, "w": 220, "h": 110, "shape": if last { "star" } else { "round" }, "text": m, "fill": if last { "yellow" } else { "white" }, "fontSize": 18 }),
                ));
            }
            for w in shapes.windows(2) {
                t.link(&w[0], &w[1], "", "right", "left", "straight");
            }
            t.section(x + 24.0, y + 678.0, 600.0, 316.0, "Open questions");
            t.add("sticky", json!({ "x": x + 40.0, "y": y + 728.0, "w": 180, "h": 140, "text": "What do we still need to decide?", "color": "violet" }));
            t.section(x + 648.0, y + 678.0, 888.0, 316.0, "First steps");
            for (i, (label, c)) in [("To do", "gray"), ("Doing", "blue"), ("Done", "green")].iter().enumerate() {
                let fx = i as f64 * 290.0;
                t.text(x + 668.0 + fx, y + 722.0, 200.0, label, 16.0, true);
                let title = ["Book the first review", "Write the plan", "Kickoff held"][i];
                t.add("card", json!({ "x": x + 664.0 + fx, "y": y + 756.0, "w": 270, "h": 110, "title": title, "text": "", "color": c, "status": label, "assignee": "" }));
            }
        }
        _ => return None,
    }
    Some(t.out)
}
