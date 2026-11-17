//! The DXF pack: the kit's tier-0 [`BlobPack`], indexing each version into the kit's
//! structure index through the format-neutral model ([`k2_mpc_cad_model::index`]), with the
//! read verbs ([`crate::verbs`]) and the edit verbs ([`crate::edit`]) as the kit's blob verbs,
//! the pack's refusals by id ([`DXF_ERRORS`]) and how far a hold reaches per node kind. Agents
//! change a drawing through the verbs only: an agent's whole-file `write` is refused
//! (`AgentWrites::VerbsOnly`, `MPC-E715`), its verbs are admitted.

use std::io::Read;

use k2_mpc_base::errors::{ErrorEntry, Value};
use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::{
    AgentWrites, BlobMerge, BlobPack, BlobPlan, BlobVerb, BlobVerbCtx, BlobVerbOut,
    BlobVerbRefused, HoldReach, SaveFacts, SavePairing, VerbInput, VerbMerge,
};
use k2_mpc_cad_model::merge::MergeOutcome;

use crate::error::{CadError, USAGE};
use crate::{mutant, read_doc, Limits};

/// The index's output format: bump when what a file indexes to changes, and the kit recomputes.
pub const FORMAT: u32 = 3;

/// The index verbs, under the noun `entity`.
pub const VERBS: IndexVerbs = IndexVerbs::new(
    "entity list",
    "entity show",
    "entity diff",
    "entity blame",
    None,
);

/// Reads ASCII DXF files.
#[derive(Debug, Clone, Copy, Default)]
pub struct DxfPack;

/// The index draft of `bytes`.
pub fn index_bytes(bytes: &[u8], limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let doc = read_doc(bytes, &Limits::default())
        .map_err(|e| IndexRefused::Unreadable { why: e.summary() })?;
    k2_mpc_cad_model::index::draft(&doc, limits)
}

impl BlobPack for DxfPack {
    /// Agents change a drawing through the verbs only: the kit refuses an agent's whole-file
    /// `write` and `write --base` (`MPC-E715`, the file unchanged) and admits its verbs, which go
    /// through the pack's rules and `--base` per entity. A person's write and any restore are
    /// unaffected.
    fn agent_writes(&self) -> AgentWrites {
        if mutant::active("AgentWritesAllowed") {
            return AgentWrites::Allowed;
        }
        if mutant::active("AgentWritesRefused") {
            return AgentWrites::Refused;
        }
        AgentWrites::VerbsOnly
    }

    /// The merge by unit and field ([`crate::merge`]).
    fn can_merge(&self) -> bool {
        true
    }

    /// A person's app saved `theirs` over a version the doc has moved on from: merged by unit
    /// and field, else both kept (a conflict copy). Never one side's value over the other's.
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        match crate::merge::merge(base, ours, theirs) {
            MergeOutcome::Merged { bytes, .. } => BlobMerge::Merged(bytes),
            MergeOutcome::Conflict(_) => BlobMerge::Conflict,
            MergeOutcome::Unsupported(_) => BlobMerge::Unsupported,
        }
    }

    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("dxf", FORMAT, &VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let rl = Limits::default();
        let mut bytes = Vec::new();
        src.take(rl.max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(IndexRefused::Read)?;
        if bytes.len() as u64 > rl.max_bytes {
            return Err(IndexRefused::Unreadable {
                why: format!(
                    "DXF-E003 over_bound: over the size limit of {} bytes",
                    rl.max_bytes
                ),
            });
        }
        // the model holds what the index needs: the bytes go before the draft is built
        let doc =
            read_doc(&bytes, &rl).map_err(|e| IndexRefused::Unreadable { why: e.summary() })?;
        drop(bytes);
        k2_mpc_cad_model::index::draft(&doc, limits)
    }

    fn verbs(&self) -> &'static [BlobVerb] {
        VERB_SET
    }

    fn errors(&self) -> &'static [ErrorEntry] {
        DXF_ERRORS
    }

    /// A hold on a layer reaches every entity on it (its `layer_key`); on a block definition,
    /// model space or a layout, everything in it; on anything else, the node alone.
    fn hold_reach(&self, kind: &str) -> HoldReach {
        match kind {
            "layer" => HoldReach::Members("layer_key"),
            "block" | "model" | "layout" => HoldReach::Subtree,
            _ => HoldReach::Node,
        }
    }

    /// Not known. DXF handles are persistent by the format's rules, but no writer's save is
    /// proven to keep them here yet (the save pairs of each writer decide that, by its mark,
    /// later): the kit judges each key across another program's save, and a `--base` call across
    /// one is refused (`MPC-E719`), so an agent reads again once.
    fn save_pairing(&self, _facts: &SaveFacts<'_>) -> SavePairing {
        SavePairing::Unknown
    }
}

// ---------------------------------------------------------------------------------------------
// refusals
// ---------------------------------------------------------------------------------------------

/// The pack's own refusals (`DXF-E…`), with what to do next: the reader's, the shared model's
/// under the pack's name (`CAD-E0nn` is `DXF-E0nn` here), and the edit verbs'.
pub static DXF_ERRORS: &[ErrorEntry] = &[
    ErrorEntry::pack(
        "DXF-E001",
        "binary_dxf",
        2,
        "the file is binary DXF",
        "Save the drawing as ASCII DXF in the app that made it.",
    ),
    ErrorEntry::pack(
        "DXF-E002",
        "not_dxf",
        2,
        "the file isn't code/value pairs",
        "The file is damaged or not a DXF file: save it again in the app that made it.",
    ),
    ErrorEntry::pack(
        "DXF-E003",
        "over_bound",
        2,
        "the file is beyond this reader's limits",
        "The drawing stays a plain file; split it, or ask for higher limits.",
    ),
    ErrorEntry::pack(
        "DXF-E010",
        "address_not_found",
        4,
        "an address names nothing in this version",
        "List addresses with entity find, layer list or block list.",
    ),
    ErrorEntry::pack(
        "DXF-E011",
        "name_ambiguous",
        2,
        "a name matches two records",
        "Name the record by its address (@h:HANDLE).",
    ),
    ErrorEntry::pack(
        "DXF-E012",
        "not_a_boundary",
        2,
        "the address doesn't name a closed polyline",
        "Name a closed polyline as the boundary.",
    ),
    ErrorEntry::pack(
        "DXF-E013",
        "stale_cursor",
        2,
        "the page cursor is from another version or space",
        "Start the pages again without --cursor.",
    ),
    ErrorEntry::pack(
        "DXF-E020",
        "layer_missing",
        2,
        "a layer the call names isn't in the drawing",
        "List the layers with layer list.",
    ),
    ErrorEntry::pack(
        "DXF-E021",
        "entity_not_editable",
        2,
        "the entity is of a kind or in a state the edit verbs don't change",
        "entity props says why; change it in the app.",
    ),
    ErrorEntry::pack(
        "DXF-E022",
        "unit_mismatch",
        2,
        "--unit isn't the drawing's unit",
        "Give the drawing's unit (drawing info names it).",
    ),
    ErrorEntry::pack(
        "DXF-E027",
        "referenced",
        2,
        "another object points at what a delete would remove",
        "Remove the reference first (the message names the object).",
    ),
    ErrorEntry::pack(
        "DXF-E040",
        "version_unsupported",
        2,
        "the edit verbs write DXF 2000 (AC1015) and later",
        "Save the drawing as DXF 2000 or later in the app, then retry.",
    ),
    ErrorEntry::pack(
        "DXF-E041",
        "handles_off",
        2,
        "the drawing can't be keyed by handle",
        "Save the drawing again in the app: it gives every object one handle.",
    ),
    ErrorEntry::pack(
        "DXF-E042",
        "external_reference",
        2,
        "the entity or layer belongs to an external reference",
        "Edit the referenced drawing, then reload it in the app.",
    ),
    ErrorEntry::pack(
        "DXF-E043",
        "proxy",
        2,
        "the entity is another application's object (a proxy)",
        "Edit it in the application that made it.",
    ),
    ErrorEntry::pack(
        "DXF-E044",
        "dynamic_geometry",
        2,
        "the entity is inside a dynamic block's definition or drawn form",
        "Change the block's parameters in the app.",
    ),
    ErrorEntry::pack(
        "DXF-E045",
        "layer_locked",
        2,
        "the entity, or the layer a change puts it on, is locked",
        "Unlock the layer in the app, then retry.",
    ),
    ErrorEntry::pack(
        "DXF-E046",
        "in_block_definition",
        2,
        "the entity is inside a block definition: a change there changes every reference",
        "Edit the block in the app's block editor.",
    ),
    ErrorEntry::pack(
        "DXF-E047",
        "attrib_missing",
        2,
        "the block reference has no attribute with that tag",
        "List its attributes with block show ADDR --attribs.",
    ),
    ErrorEntry::pack(
        "DXF-E048",
        "attrib_driven",
        2,
        "the attribute's value is a field the app computes",
        "Change the field's source in the app.",
    ),
    ErrorEntry::pack(
        "DXF-E049",
        "attrib_multiline",
        2,
        "a multi-line attribute, which this release doesn't write",
        "Edit it in the app.",
    ),
    ErrorEntry::pack(
        "DXF-E050",
        "edit_too_many",
        2,
        "more entities than one call may change",
        "Split the call.",
    ),
    ErrorEntry::pack(
        "DXF-E053",
        "table_entry_missing",
        2,
        "a linetype or text style the call names isn't in its table",
        "Name one the drawing has.",
    ),
    ErrorEntry::pack(
        "DXF-E054",
        "value_not_writable",
        2,
        "a value the format can't hold in that place",
        "Give a value that fits (one line, at most 255 bytes).",
    ),
    ErrorEntry::pack(
        "DXF-E090",
        "self_check",
        2,
        "the edit didn't read back as planned; nothing was written",
        "Report it with the file; the file is unchanged.",
    ),
];

/// The id a refusal has under the pack's name: a shared `CAD-E0nn` is `DXF-E0nn`.
pub fn pack_id(id: &str) -> String {
    match id.strip_prefix("CAD-E") {
        Some(n) => format!("DXF-E{n}"),
        None => id.to_owned(),
    }
}

/// A refusal as the kit takes it: a usage refusal as usage (exit 2), any other by its entry.
pub fn refused(e: CadError) -> BlobVerbRefused {
    if e.code == USAGE {
        return BlobVerbRefused::usage(format!("{}; {}", e.why, e.next));
    }
    let id = pack_id(e.code.id);
    match DXF_ERRORS.iter().find(|x| x.id == id) {
        Some(entry) => BlobVerbRefused::pack(entry, e.why, e.next),
        None => BlobVerbRefused::usage(e.summary()),
    }
}

// ---------------------------------------------------------------------------------------------
// the verbs
// ---------------------------------------------------------------------------------------------

const READ: &[PersonAction] = &[PersonAction::ReadAll];

/// Runs read verb `verb` on the version the kit hands it. A drawing read (`draw header`,
/// `draw page`) reads that version's number as its `--version`: given, it must be that one.
fn read_verb(
    ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
    verb: &str,
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .map_err(|_| BlobVerbRefused::usage("the version can't be read"))?;
    let mut a: Vec<String> = args.to_vec();
    if verb == "draw header" || verb == "draw page" {
        let v = ctx.version().to_string();
        match a.iter().position(|x| x == "--version") {
            Some(i) => {
                if a.get(i + 1) != Some(&v) {
                    return Err(BlobVerbRefused::usage(format!(
                        "{verb} reads the version the call is on (v{v}); --version must be {v}"
                    )));
                }
            }
            None => {
                a.push("--version".into());
                a.push(v);
            }
        }
    }
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    let o = crate::verbs::run(&bytes, verb, &refs).map_err(refused)?;
    let mut fields = vec![(
        "schema".to_owned(),
        Value::from(format!("{}/{}", o.spec.0, o.spec.1)),
    )];
    match o.data {
        Value::Obj(kv) => fields.extend(kv),
        other => fields.push(("value".into(), other)),
    }
    if let Some(next) = o.next {
        fields.push((
            "next".into(),
            Value::List(next.into_iter().map(Value::from).collect()),
        ));
    }
    Ok(BlobVerbOut::read(o.stdout).with_data(Value::Obj(fields)))
}

macro_rules! read_verbs {
    ($($f:ident => $name:literal, $usage:literal;)*) => {
        $(
            fn $f(
                ctx: &BlobVerbCtx<'_>,
                src: &mut dyn Read,
                args: &[String],
            ) -> Result<BlobVerbOut, BlobVerbRefused> {
                read_verb(ctx, src, args, $name)
            }
        )*
        /// The read verbs, as the kit lists them.
        const READ_VERBS: &[BlobVerb] = &[
            $(BlobVerb::read($name, $usage, READ, VerbInput::Whole, $f).with_exits(&[0, 2, 4]),)*
        ];
    };
}

read_verbs! {
    r_info => "drawing info", "(version, units, extents, counts, handles, what is editable)";
    r_layers => "layer list", "[--unused]";
    r_blocks => "block list", "[--unused] [--anon]";
    r_find => "entity find", "(at least one filter) [--layer L] [--kind K] [--ltype T] [--in model|paper|@h:B] [--window x0,y0,x1,y1 | --crossing x0,y0,x1,y1] [--inside @h:B | --crosses @h:B] [--after @h:H] [--limit N]";
    r_text => "text find", "--contains S [--layer L] [--case] [--after @h:H] [--limit N]";
    r_attribs => "attrib find", "[--tag T] [--value V] [--block B] [--in SPACE] [--after @h:H] [--limit N]";
    r_block_show => "block show", "@h:INSERT [--attribs]";
    r_props => "entity props", "@h:H...";
    r_header => "header get", "$VAR...";
    r_check => "check", "[--writeback]";
    r_draw_header => "draw header", "[--version N] (the version read)";
    r_draw_page => "draw page", "--space model|paper|@h:B [--cursor C] [--version N]";
}

const EDIT: &[PersonAction] = &[PersonAction::Rewrite];

/// Plans edit verb `verb` on the version the kit hands it: the plan, the entities it changes,
/// removes and adds, and where its result is.
fn edit_verb(
    src: &mut dyn Read,
    args: &[String],
    verb: &str,
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .map_err(|_| BlobVerbRefused::usage("the version can't be read"))?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let p = crate::edit::plan(&bytes, verb, &refs).map_err(refused)?;
    let mut plan = BlobPlan::new();
    for sp in p.spans {
        plan = plan.span(sp.lo as u64, (sp.hi - sp.lo) as u64, sp.with);
    }
    let mut out = BlobVerbOut::edit(plan, p.stdout).with_data(p.data);
    for k in p.touched {
        out = out.touching(k);
    }
    for k in p.created {
        out = out.creating(k);
    }
    if let Some(v) = p.viewing {
        out = out.viewing(v);
    }
    Ok(out)
}

macro_rules! edit_verbs {
    ($($f:ident => $name:literal, $word:literal, $usage:literal;)*) => {
        $(
            fn $f(
                _ctx: &BlobVerbCtx<'_>,
                src: &mut dyn Read,
                args: &[String],
            ) -> Result<BlobVerbOut, BlobVerbRefused> {
                edit_verb(src, args, $name)
            }
        )*
        /// The edit verbs, as the kit lists them: each one the pack's merge covers.
        const EDIT_VERBS: &[BlobVerb] = &[
            $(BlobVerb::edit($name, $usage, EDIT, VerbMerge::Pack, VerbInput::Whole, $f)
                .with_word($word)
                .with_exits(&[0, 2, 4]),)*
        ];
    };
}

edit_verbs! {
    e_attrib_set => "attrib set", "set", "@h:INSERT --tag T --value V [--nth N]";
    e_set => "entity set", "changed", "@h:E... [--layer L] [--color N|bylayer|byblock] [--rgb #rrggbb] [--ltype NAME|bylayer|byblock]";
    e_move => "entity move", "moved", "@h:E... --by dx,dy --unit U";
    e_add => "entity add", "added", "line --from x,y[,z] --to x,y[,z] | polyline --points x,y;x,y;... [--closed] [--bulges b,...] [--width W] | text --at x,y[,z] --height H --value S [--rotate DEG] [--style NAME] [--align left|center|right]; --unit U [--layer L] [--color N] [--ltype NAME] [--in model|paper|@h:LAYOUT]";
    e_delete => "entity delete", "deleted", "@h:E... (its followers and what it owns with it)";
}

/// Every verb the pack gives its blob docs: the reads, then the edits.
pub static VERB_SET: &[BlobVerb] = &[
    READ_VERBS[0],
    READ_VERBS[1],
    READ_VERBS[2],
    READ_VERBS[3],
    READ_VERBS[4],
    READ_VERBS[5],
    READ_VERBS[6],
    READ_VERBS[7],
    READ_VERBS[8],
    READ_VERBS[9],
    READ_VERBS[10],
    READ_VERBS[11],
    EDIT_VERBS[0],
    EDIT_VERBS[1],
    EDIT_VERBS[2],
    EDIT_VERBS[3],
    EDIT_VERBS[4],
];
