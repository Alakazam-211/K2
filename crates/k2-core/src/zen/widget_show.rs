//! `k2 zen widget show <name>` (Garden v3, contract-sources-v1 §4.2, §7):
//! what one widget is doing, for the agent that builds it. Each placement
//! with its Garden, whether that Garden is locked, whether the runaway
//! guard paused it, and the source addresses it reads; the caps it asked
//! for and the caps it runs with, with the reach note when a widget that
//! reads sources may also post or save (0.46.1, `sources_reach_note`); and
//! its runtime record for the version on disk now (`widget_runs`).

use serde_json::{json, Value as J};

use crate::zen::builtin_widgets::{parse_widget_ref, WidgetRef};
use crate::zen::store::{ZenError, ZenFile, ZenFiles};
use crate::zen::widget_access::{effective_caps, sources_reach_note, WidgetOrigin, WidgetPauses};
use crate::zen::widget_store::widget_label;
use crate::zen::widgets;

impl ZenFiles {
    /// GET `/cli/zen/widget/show?widget=<name|k2:ref>`.
    pub fn widget_show(&self, widget: &str, pauses: &WidgetPauses) -> Result<J, ZenError> {
        self.require_set_up()?;
        let widget = widget.trim();
        let name = match parse_widget_ref(widget) {
            Ok(WidgetRef::User(n)) => n,
            Ok(WidgetRef::Builtin { .. }) => widget.to_string(),
            Err(m) => return Err(ZenError::BadRequest(m)),
        };
        let st = self.widget_status(&name);
        let placed: Vec<_> = self.custom_placements().into_iter().filter(|p| p.widget == name).collect();
        if !st.exists && st.manifest.is_none() && placed.is_empty() {
            return Err(ZenError::NotFound(format!("{} has no folder and isn't placed", widget_label(&name))));
        }
        let requested = st.requested();
        let origin = WidgetOrigin::of(&name);
        let caps = effective_caps(origin, &requested);
        // Asked for but not run with (a cap this K2 doesn't give widgets).
        let dropped: Vec<&str> = requested.iter().filter(|c| !caps.contains(c)).map(String::as_str).collect();
        let reach = sources_reach_note(&caps);
        let sources = st.manifest.as_ref().and_then(|m| m.sources.clone());
        let reads = st.manifest.as_ref().map(|m| m.reads.clone()).unwrap_or_default();

        let mut placements = Vec::new();
        for p in &placed {
            let f = ZenFile::Garden(p.garden.clone());
            let eff = self.effective(&f, &self.base_for(&f));
            let page = crate::zen::garden_page(&eff.layer, &self.gardens().iter().find(|g| g.id == p.garden).map(|g| g.template.clone()).unwrap_or_default());
            let config = page["widgets"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| w["id"] == p.placement.as_str())
                .and_then(|w| w["props"].get("config").cloned())
                .unwrap_or_else(|| json!({}));
            let gst = self.garden_widget_status(&p.garden, &name);
            let gsources = gst.manifest.as_ref().and_then(|m| m.sources.clone());
            let paused = pauses.get(&p.garden, &p.placement);
            let locked = self.is_locked(&p.garden);
            let state = match (gst.state.as_str(), paused.is_some()) {
                ("broken", _) => "stopped: it doesn't bundle (k2 zen validate --widget shows why)",
                (_, true) => "running; posting paused by the runaway guard (the person clicks Resume)",
                ("errors", _) => "running its last good version (the folder has errors)",
                _ => "running when a Zen window shows its Garden",
            };
            placements.push(json!({
                "garden": p.garden,
                "gardenName": p.garden_name,
                "placement": p.placement,
                "locked": locked,
                "paused": paused,
                "state": gst.state.as_str(),
                "note": state,
                "sources": widgets::placement_source_addresses(gsources.as_ref(), &config),
            }));
        }

        let runtime = match (parse_widget_ref(&name), st.bundle.as_ref()) {
            (Ok(WidgetRef::User(n)), Some(b)) => {
                let s = self.widget_run_summary(&n, &b.hash);
                json!({ "hash": b.hash, "ran": s.ran, "errorsSinceSave": s.errors })
            }
            _ => J::Null,
        };
        Ok(json!({
            "ok": true,
            "widget": name,
            "name": st.title(),
            "state": st.state.as_str(),
            "requested": requested,
            "caps": caps,
            "dropped": dropped,
            "reach": reach,
            "sources": sources,
            "reads": reads,
            "placements": placements,
            "runtime": runtime,
        }))
    }
}
