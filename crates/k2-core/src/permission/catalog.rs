//! The permission-card catalog: one [`ActionSpec`] per thing an agent may
//! ask a person to change (PRD rev 3.1 §5; Appendix A).
//!
//! The v1 catalog (§5.2) is the Agent tab's switches plus the important held
//! requests plus Keep as mine: 24 card-able rows, in two lanes:
//! - **E1** (`k2-daemon/src/permission/actions/{switches,connections,gardens}.rs`):
//!   17 switch actions (vault, DNS and mail manage / send mode included; they
//!   are Agent-tab switches), the `to=off` twins and `garden.keep-mine`;
//! - **E2** (`actions/{held,compute}.rs`): `apps.approve`, `mail.approve`,
//!   `garden.proposal.approve`, `sources.approve` (plain sources only) and
//!   `compute.admin-approve` (cut if late).
//!
//! Garden access (prd-garden-access-v1 G16, 0.46.3 behind `gardenAccess`,
//! default off) adds three rows in `actions/gardens.rs`: `garden.access`,
//! its `garden.access-off` twin and `garden.delete`. While the switch is
//! off they are listed and their checks answer `not_available`; while it
//! is on, `garden.keep-mine` and `garden.proposal.approve` answer `retired`.
//!
//! The specs are static data. The daemon side pairs each with `check` and
//! `apply`; `apply` calls the **route-level entry** with the answerer as the
//! actor, so the route's own floor check runs again at the click (§6.4).
//!
//! P0: types only. No action is registered yet.

use serde::{Deserialize, Serialize};

/// The 13 areas of Appendix A (most rows outside the v1 catalog are
/// dropped, later, covered or never; the areas still classify them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Area {
    /// A1 · agent switches (Workspace → Agent tab) and connections.
    Switches,
    /// A2 · held requests and proposals that already have an id.
    Held,
    /// A3 · Apps, other than switches.
    Apps,
    /// A4 · compute, other than switches.
    Compute,
    /// A5 · the vault.
    Vault,
    /// A6 · Sew.
    Sew,
    /// A7 · mail.
    Mail,
    /// A8 · DNS, domains and certs.
    Domains,
    /// A9 · teams and hire.
    Teams,
    /// A10 · clone.
    Clone,
    /// A11 · Gardens and data sources.
    Gardens,
    /// A12 · everything else.
    Misc,
    /// A13 · box-level.
    Box,
}

impl Area {
    pub const ALL: [Area; 13] = [
        Area::Switches,
        Area::Held,
        Area::Apps,
        Area::Compute,
        Area::Vault,
        Area::Sew,
        Area::Mail,
        Area::Domains,
        Area::Teams,
        Area::Clone,
        Area::Gardens,
        Area::Misc,
        Area::Box,
    ];

    /// Wire / CLI name (`k2 permission list --area <a>`).
    pub fn as_str(self) -> &'static str {
        match self {
            Area::Switches => "switches",
            Area::Held => "held",
            Area::Apps => "apps",
            Area::Compute => "compute",
            Area::Vault => "vault",
            Area::Sew => "sew",
            Area::Mail => "mail",
            Area::Domains => "domains",
            Area::Teams => "teams",
            Area::Clone => "clone",
            Area::Gardens => "gardens",
            Area::Misc => "misc",
            Area::Box => "box",
        }
    }

    pub fn parse(s: &str) -> Option<Area> {
        Area::ALL.into_iter().find(|a| a.as_str() == s)
    }

    /// The PRD's Appendix A section (`A1`…`A13`).
    pub fn appendix(self) -> &'static str {
        match self {
            Area::Switches => "A1",
            Area::Held => "A2",
            Area::Apps => "A3",
            Area::Compute => "A4",
            Area::Vault => "A5",
            Area::Sew => "A6",
            Area::Mail => "A7",
            Area::Domains => "A8",
            Area::Teams => "A9",
            Area::Clone => "A10",
            Area::Gardens => "A11",
            Area::Misc => "A12",
            Area::Box => "A13",
        }
    }
}

/// The type of one action value. Every value is checked to exist and to
/// belong to the caller at request time and again at the click (§3.2,
/// §6.2). The workspace always comes from the passport, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParamKind {
    /// Another workspace (handle): `connections.add to=`.
    Workspace,
    /// An App core (`db`, `files`, `thread`, `mail`, `sew`): `apps.core core=`.
    Core,
    /// A held request that already exists (`ar_…`, a node-admin request).
    HeldRequest,
    /// A held mail message id: `mail.approve message=`.
    Message,
    /// A Garden proposal id: `garden.proposal.approve proposal=`.
    Proposal,
    /// A Garden data source id: `sources.approve source=`.
    Source,
    /// A Garden widget id: `garden.keep-mine widget=`.
    Widget,
    /// A widget version hash: `garden.keep-mine version=`.
    Version,
    /// A Garden id (`garden-2`): `garden.access garden=`, `garden.delete
    /// garden=` (prd-garden-access-v1 G16). The card's words show the
    /// Garden's **current** name, never the id; a rename between the ask
    /// and the click doesn't make the request stale.
    Garden,
    /// One of a fixed set of words (`mail.send-mode mode=`,
    /// `db.agent-access level=`); see [`ParamSpec::choices`].
    Choice,
    /// A compute node, by name: `compute.ci node=` (prd-compute-job-tiers-v1
    /// T13).
    Node,
    /// A vault secret of the asking workspace, by name (never a value):
    /// `vault.delivery secret=` (2026-10-28).
    Secret,
    /// A heartbeat, by name, of the workspace another param names:
    /// `heartbeat.pause-other target= heartbeat=` (0.46.6).
    Heartbeat,
    /// An App user (a skin principal), by username, never an email:
    /// `apps.user-password-reset user=` (0.46.6). Checked to hold a grant
    /// on an App of the asking workspace.
    AppUser,
    /// An App of the asking workspace (its id, name or `<ws>/<name>`):
    /// `apps.user-password-reset app=` (0.46.6).
    App,
    /// A worktree sidecar of the asking workspace, by handle (its
    /// `<ws>/<name>` address's name): `git.land sidecar=` (0.46.7).
    Sidecar,
    /// A git commit, its full sha or a prefix of at least 7: `git.land
    /// tip=`, the branch tip the person lands (0.46.7).
    Commit,
}

/// One value an action takes (`key=value` on the CLI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub key: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// A comma list (`a,b`; `\,` is a literal comma — the
    /// `core/overlay/options.rs` splitter).
    pub list: bool,
    /// The allowed words for [`ParamKind::Choice`]; empty otherwise.
    pub choices: &'static [&'static str],
}

/// Who may answer: **the code's real floor** for the Settings route
/// (§5.1, Appendix A). A parity test walks the catalog and checks each
/// floor against the route's own gate through both doors (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Floor {
    /// The owner token or an Owner login.
    Owner,
    /// The owner token, an Owner login or an Admin login.
    Admin,
    /// The owner of that Garden from any signed-in surface: the owner token
    /// or a Connect login the owner linked, whatever its role (phone, web,
    /// another server; GP-B1). Resolved by `zen::garden_owner_may`, which
    /// Settings calls too (§5.2.1).
    GardenOwner,
}

impl Floor {
    /// `view.approvers.floor` (§17.2.1): `owner` or `admin`.
    pub fn as_str(self) -> &'static str {
        match self {
            Floor::Owner | Floor::GardenOwner => "owner",
            Floor::Admin => "admin",
        }
    }

    /// `view.approvers.label` and the refusal's `approvers` line.
    pub fn label(self) -> &'static str {
        match self {
            Floor::Owner | Floor::GardenOwner => "owner",
            Floor::Admin => "owner or admin",
        }
    }
}

/// Loosens or tightens access ("Loosens access" is drawn amber).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Loosen,
    Tighten,
}

/// A standing grant (stays until someone turns it off) or a one-time act
/// (`view.grant`, §17.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grant {
    Standing,
    OneTime,
}

/// The existing `CONFIRM_ACTIONS` entries (`k2-daemon/src/vault_routes.rs`)
/// a v1 card takes — exactly the ones Settings takes for the same act
/// (§5.3). No card binds a secret; `vault-move` (0.46.6) binds a held
/// box-move step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfirmAction {
    /// Approving a held App request; bound to the request id.
    #[serde(rename = "app-approve")]
    AppApprove,
    /// A node root step; bound to the request id.
    #[serde(rename = "node-admin")]
    NodeAdmin,
    /// A held box-move step (`mv_…`, prd-box-move-carry-v1 Q3); bound to
    /// the held id.
    #[serde(rename = "vault-move")]
    VaultMove,
    /// Opening Sew's web app as its install owner (Rosson 2026-10-28, the
    /// card `sew.owner-handoff`); bound to the request id.
    #[serde(rename = "chat-owner-handoff")]
    ChatOwnerHandoff,
    /// The emergency Sew rollback across a schema change (Rosson
    /// 2026-10-28, the card `sew.rollback-data-loss`); bound to the request
    /// id.
    #[serde(rename = "sew-rollback-data-loss")]
    SewRollbackDataLoss,
}

impl ConfirmAction {
    pub const ALL: [ConfirmAction; 5] = [
        ConfirmAction::AppApprove,
        ConfirmAction::NodeAdmin,
        ConfirmAction::VaultMove,
        ConfirmAction::ChatOwnerHandoff,
        ConfirmAction::SewRollbackDataLoss,
    ];

    /// The `CONFIRM_ACTIONS` string (`vaultApi.confirmStart(<this>, …)`).
    pub fn as_str(self) -> &'static str {
        match self {
            ConfirmAction::AppApprove => "app-approve",
            ConfirmAction::NodeAdmin => "node-admin",
            ConfirmAction::VaultMove => "vault-move",
            ConfirmAction::ChatOwnerHandoff => "chat-owner-handoff",
            ConfirmAction::SewRollbackDataLoss => "sew-rollback-data-loss",
        }
    }
}

/// The proof the answer needs: exactly what Settings asks (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Proof {
    /// A plain click (a second tap on the phone).
    Click,
    /// The fresh confirm Settings already takes for this act.
    SettingsConfirm { action: ConfirmAction },
}

impl Proof {
    /// "Allow all" in a batch covers click items only (§4.5).
    pub fn batchable(self) -> bool {
        matches!(self, Proof::Click)
    }
}

/// Default card expiry: 72 h (§3.3).
pub const DEFAULT_EXPIRY_SECS: i64 = 72 * 3600;
/// Maximum card expiry: 7 days. Never "never".
pub const MAX_EXPIRY_SECS: i64 = 7 * 24 * 3600;

/// How long a card stays answerable. The request's expiry **is** the
/// card's `expiresAt` (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Expiry {
    pub default_secs: i64,
    pub max_secs: i64,
}

impl Expiry {
    pub const STANDARD: Expiry = Expiry { default_secs: DEFAULT_EXPIRY_SECS, max_secs: MAX_EXPIRY_SECS };

    /// Positive, default ≤ max ≤ 7 days.
    pub fn valid(self) -> bool {
        self.default_secs > 0 && self.default_secs <= self.max_secs && self.max_secs <= MAX_EXPIRY_SECS
    }
}

/// How an approved standing grant is undone ("Turn off again" for 7 days,
/// `POST /cli/thread/permission-undo`, §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Undo {
    /// One-time acts and tightenings: nothing to undo from the card.
    None,
    /// The tighten twin, run through the same route entry with no confirm.
    Twin { action: &'static str },
}

/// Where the same control lives in Settings ("Open in Settings").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRef {
    /// `view.settingsPage`: the deep-link id (`workspaces/{ws}/agent`).
    pub page: &'static str,
    /// Human path for refusals ("Settings → Workspaces → {ws} → Agent").
    pub path: &'static str,
}

/// K2-written words. Templates take daemon-known names only: `{ws}`,
/// `{agent}` and the action's own param keys in braces. The agent's `--why`
/// is never part of them (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Words {
    /// `view.setting`: the Settings label ("Allow compute nodes").
    pub setting: &'static str,
    /// `view.whatChanges`: K2's sentence ("Let {ws} run jobs on this
    /// server's compute nodes").
    pub what_changes: &'static str,
    /// `view.consequences`: K2's plain words about the effect.
    pub consequences: &'static str,
    /// `view.scope` template ("{ws}", "request {request} (service {app})").
    pub scope: &'static str,
    /// `view.duration`: "until someone turns it off" or "once".
    pub duration: &'static str,
    /// The approved turn's tail: "run your command again" (Q9; never the
    /// agent's command line, C5).
    pub retry: &'static str,
}

/// What a refusal carries that makes it this action's wall (§3.1, §5.1).
/// Matched by the central refusal wrapper (lane C); no wall's handler is
/// edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum WallMatch {
    /// A refusal with this `error.code` (or flat `error` string).
    Code { status: u16, code: &'static str },
    /// A held answer: a 2xx whose JSON `field` equals `value`
    /// (`{202, "status", "held"}`, or `"pending_approval"` for a held send).
    Held { status: u16, field: &'static str, value: &'static str },
}

/// HTTP method a wall answers on; `Any` when the route is method-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum WallMethod {
    Get,
    Post,
    Any,
}

/// Where a wall's param value comes from in the refused request (read
/// from the request-scoped task-local, §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "from", content = "key")]
pub enum ParamFrom {
    /// A query or form parameter.
    Query(&'static str),
    /// A JSON pointer into the request body (`/node`).
    Body(&'static str),
    /// A JSON pointer into the refusal body (`/request/id`).
    Response(&'static str),
}

/// One wall of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wall {
    pub matches: WallMatch,
    /// Route prefix (`/cli/compute/`); generic codes (`owner_only`,
    /// `gated`, `forbidden`, `person_only`) need prefix + method.
    pub route_prefix: &'static str,
    pub method: WallMethod,
    /// `(param key, where its value comes from)`.
    pub params: &'static [(&'static str, ParamFrom)],
}

/// How well an action's refusals can teach (Appendix A's coverage column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallCoverage {
    /// A refusal with its own code.
    Code,
    /// A held 2xx answer.
    Held,
    /// A generic code, told apart by route prefix + method only.
    PrefixOnly,
    /// No refusal at all: reached only by asking (`k2 permission list`).
    None,
}

/// A refusal code that means "done at a machine" (§5.5). Used by the
/// refusal wrapper only: the body gets `permission: {requestable:false,
/// where:"at the machine"}` and the CLI prints the hint before "It can't be
/// requested." There is no card, route, view or state for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineHint {
    /// The refusal code (`remote_admin_off`).
    pub code: &'static str,
    /// The plain-text hint; daemon-known names only (`{node}`): "At {node},
    /// run: `sudo k2-node admin level policy` (switch back after: `sudo
    /// k2-node admin level off`)".
    pub hint: &'static str,
}

/// Why something can never be a card (§9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NeverReason {
    /// It can't be undone.
    Irreversible,
    /// Root or box-level, or it changes who can reach the box (incl.
    /// raising a node's remote admin level: machine-only).
    BoxLevel,
    /// It reveals or mints a secret, or touches the person-proof
    /// (`garden-guests`, `clone-carry`).
    Secret,
    /// The person's own tool (the Garden lock, the two card switches).
    PersonalTool,
    /// Releases.
    Release,
}

/// Why a row outside the v1 catalog isn't card-able now (§5.4). Its
/// refusal says "It can't be requested." or names its covering action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum NotCardable {
    Never { reason: NeverReason },
    /// Card-able in a later release.
    Later,
    /// Dropped from v1 (Rosson Q4).
    Dropped,
    /// Covered by another action: the refusal says `Ask with: … <action>`.
    Covered { by: &'static str },
}

/// One catalog action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSpec {
    /// `area.verb` (`compute.use`).
    pub id: &'static str,
    pub area: Area,
    pub params: &'static [ParamSpec],
    pub floor: Floor,
    pub direction: Direction,
    pub grant: Grant,
    pub proof: Proof,
    pub expiry: Expiry,
    pub words: Words,
    pub undo: Undo,
    pub settings: SettingsRef,
    pub walls: &'static [Wall],
    pub coverage: WallCoverage,
    pub machine_hints: &'static [MachineHint],
}

/// A row that looks like a catalog row but isn't card-able in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotCardableSpec {
    pub id: &'static str,
    pub area: Area,
    pub why: NotCardable,
    /// Where a person does it ("Settings → …" or "a terminal outside K2").
    pub r#where: &'static str,
}

/// Static checks every registered spec must pass (lane tests call this on
/// the whole registry).
pub fn validate(spec: &ActionSpec) -> Result<(), String> {
    let (area, verb) = spec.id.split_once('.').ok_or_else(|| format!("{}: id is not area.verb", spec.id))?;
    if area.is_empty() || verb.is_empty() {
        return Err(format!("{}: id is not area.verb", spec.id));
    }
    if !spec.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-') {
        return Err(format!("{}: id must be lowercase a-z, 0-9, '.', '-'", spec.id));
    }
    if !spec.expiry.valid() {
        return Err(format!("{}: expiry must be positive and at most 7 days", spec.id));
    }
    let mut keys: Vec<&str> = spec.params.iter().map(|p| p.key).collect();
    keys.sort_unstable();
    if keys.windows(2).any(|w| w[0] == w[1]) {
        return Err(format!("{}: duplicate param key", spec.id));
    }
    if keys.contains(&"workspace") {
        return Err(format!("{}: the workspace comes from the passport, never a param", spec.id));
    }
    for p in spec.params {
        if (p.kind == ParamKind::Choice) == p.choices.is_empty() {
            return Err(format!("{}: param {} has choices iff it is a Choice", spec.id, p.key));
        }
    }
    if spec.words.setting.trim().is_empty() || spec.words.what_changes.trim().is_empty() {
        return Err(format!("{}: the card needs a setting label and a what-changes line", spec.id));
    }
    if matches!(spec.coverage, WallCoverage::None) != spec.walls.is_empty() {
        return Err(format!("{}: coverage None iff there are no walls", spec.id));
    }
    if matches!(spec.proof, Proof::SettingsConfirm { .. }) && spec.direction == Direction::Tighten {
        return Err(format!("{}: lowering never takes a confirm in Settings", spec.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USE: ActionSpec = ActionSpec {
        id: "compute.use",
        area: Area::Switches,
        params: &[],
        floor: Floor::Admin,
        direction: Direction::Loosen,
        grant: Grant::Standing,
        proof: Proof::Click,
        expiry: Expiry::STANDARD,
        words: Words {
            setting: "Allow compute nodes",
            what_changes: "Let {ws} run jobs on this server's compute nodes",
            consequences: "Every active machine; default limits; jobs see only the code {ws} sends.",
            scope: "{ws}",
            duration: "until someone turns it off",
            retry: "run your command again",
        },
        undo: Undo::Twin { action: "compute.use-off" },
        settings: SettingsRef { page: "workspaces/{ws}/agent", path: "Settings → Workspaces → {ws} → Agent" },
        walls: &[Wall {
            matches: WallMatch::Code { status: 403, code: "compute_off" },
            route_prefix: "/cli/compute/",
            method: WallMethod::Any,
            params: &[],
        }],
        coverage: WallCoverage::Code,
        machine_hints: &[],
    };

    #[test]
    fn a_well_formed_spec_validates() {
        validate(&USE).expect("valid");
    }

    #[test]
    fn a_workspace_param_is_refused() {
        let mut s = USE;
        s.params = &[ParamSpec { key: "workspace", kind: ParamKind::Workspace, required: true, list: false, choices: &[] }];
        let e = validate(&s).expect_err("workspace param must be refused");
        assert!(e.contains("passport"), "{e}");
    }

    #[test]
    fn choice_params_need_choices() {
        let mut s = USE;
        s.params = &[ParamSpec { key: "mode", kind: ParamKind::Choice, required: true, list: false, choices: &[] }];
        assert!(validate(&s).is_err());
        s.params =
            &[ParamSpec { key: "mode", kind: ParamKind::Choice, required: true, list: false, choices: &["approval", "free"] }];
        validate(&s).expect("a Choice with choices");
    }

    #[test]
    fn expiry_is_never_never() {
        let mut s = USE;
        s.expiry = Expiry { default_secs: DEFAULT_EXPIRY_SECS, max_secs: MAX_EXPIRY_SECS + 1 };
        assert!(validate(&s).is_err());
        s.expiry = Expiry { default_secs: 0, max_secs: MAX_EXPIRY_SECS };
        assert!(validate(&s).is_err());
    }

    #[test]
    fn coverage_none_iff_no_walls() {
        let mut s = USE;
        s.coverage = WallCoverage::None;
        assert!(validate(&s).is_err());
        s.walls = &[];
        validate(&s).expect("None with no walls");
    }

    #[test]
    fn bad_ids_are_refused() {
        for id in ["compute", "Compute.use", ".use", "compute.", "compute use"] {
            let mut s = USE;
            s.id = id;
            assert!(validate(&s).is_err(), "{id} must be refused");
        }
    }

    #[test]
    fn a_tightening_never_takes_a_confirm() {
        let mut s = USE;
        s.direction = Direction::Tighten;
        s.proof = Proof::SettingsConfirm { action: ConfirmAction::AppApprove };
        assert!(validate(&s).is_err());
    }

    #[test]
    fn area_names_round_trip_and_cover_appendix_a() {
        let mut appendix: Vec<&str> = Area::ALL.iter().map(|a| a.appendix()).collect();
        appendix.dedup();
        assert_eq!(appendix.len(), 13);
        for a in Area::ALL {
            assert_eq!(Area::parse(a.as_str()), Some(a));
        }
        assert_eq!(Area::parse("nope"), None);
    }

    #[test]
    fn the_card_confirms_are_app_approve_node_admin_vault_move_and_the_two_sew_ones() {
        let names: Vec<&str> = ConfirmAction::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names, ["app-approve", "node-admin", "vault-move", "chat-owner-handoff", "sew-rollback-data-loss"]);
        for c in ConfirmAction::ALL {
            assert_eq!(serde_json::to_value(c).unwrap(), serde_json::Value::String(c.as_str().to_string()));
        }
        assert!(Proof::Click.batchable());
        assert!(!Proof::SettingsConfirm { action: ConfirmAction::NodeAdmin }.batchable());
    }

    #[test]
    fn a_garden_value_is_its_own_kind() {
        assert_eq!(serde_json::to_value(ParamKind::Garden).unwrap(), serde_json::json!("garden"));
        let mut s = USE;
        s.params = &[ParamSpec { key: "garden", kind: ParamKind::Garden, required: true, list: false, choices: &[] }];
        validate(&s).expect("a Garden param");
    }

    /// 0.46.6: the password-reset card's two value kinds.
    #[test]
    fn app_user_and_app_values_are_their_own_kinds() {
        assert_eq!(serde_json::to_value(ParamKind::AppUser).unwrap(), serde_json::json!("app-user"));
        assert_eq!(serde_json::to_value(ParamKind::App).unwrap(), serde_json::json!("app"));
        let mut s = USE;
        s.params = &[
            ParamSpec { key: "user", kind: ParamKind::AppUser, required: true, list: false, choices: &[] },
            ParamSpec { key: "app", kind: ParamKind::App, required: false, list: false, choices: &[] },
        ];
        validate(&s).expect("AppUser and App params");
    }

    #[test]
    fn floors_render_as_the_phone_contract_says() {
        assert_eq!((Floor::Owner.as_str(), Floor::Owner.label()), ("owner", "owner"));
        assert_eq!((Floor::Admin.as_str(), Floor::Admin.label()), ("admin", "owner or admin"));
        assert_eq!(Floor::GardenOwner.as_str(), "owner", "approvers.floor is owner or admin only");
    }
}
