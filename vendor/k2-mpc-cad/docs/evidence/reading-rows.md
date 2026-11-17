# Evidence: the reading rows and their plants

Each row below is shown able to fail by its plant: a bug planted in the code it guards
(`--features mutants`, one plant active at a time), which turns that row red at its own
assertion. `crates/dxf/tests/mutants.rs` runs every plant and fails if any row stays green.

Green run of record: commit `036a0e30`, with the kit at `58b72f9a`; every gate part passed (formatting, lints with
warnings denied, the docs with warnings denied, the suite, the plants, licences, licence
files, the private-name and label guards); "plants: 20 of 20 caught".

| Row (file) | Plant | Caught at |
|---|---|---|
| `tokenize_reemit_identical` (tokenize) | `LineEndingNormalised` | the byte comparison of the written-back tokens |
| `tokenize_reemit_identical` (tokenize) | `CodePaddingDropped` | the same |
| `tokenize_reemit_identical` (tokenize) | `CommentDropped` | the same |
| `bom_and_tail_are_carried` (tokenize) | `TailDropped` | the written-back bytes against the input |
| `not_pairs_refused` (tokenize) | `LenientTokenizer` | the refusal of a code that isn't an integer |
| `handles_normalised` (index) | `HandleNotNormalised` | the node keyed by the normalised handle |
| `hmax_from_every_object` (index) | `HmaxFromSeed` | the highest handle against the generator's |
| `no_handles_minted` (index) | `KeyFromPosition` | an entity's key in a file without handles |
| `followers_in_parent` (index) | `FollowersAsNodes` | no attribute, vertex or end marker as a node |
| `attrs_are_own_facts` (index) | `CountOnLayerNode` | the layer node's attributes after two adds |
| `move_is_not_rename` (index) | `PointInName` | the moved entity's name |
| `layer_change_is_a_field` (index) | `LayerInContentHash` | the content digest after a layer change |
| `lastsavedby_not_indexed` (index) | `HeaderCopiedWhole` | the doc attributes |
| `bbox_exact_kinds` (index) | `BulgeIgnored` | the bulged polyline's box |
| `bbox_exact_kinds` (index) | `OcsIgnored` | the mirrored circle's box |
| `compact_attrs_read_back_the_same` (index) | `DefaultNotFilled` | a node's attributes with the defaults filled back, against the full draft |
| `units_on_geometry_reads` (verbs) | `UnitsDropped` | the units of `drawing info` |
| `extents_header_vs_computed` (verbs) | `ExtentsFromHeader` | `header_stale` |
| `block_counts_nested` (verbs) | `NestedCountsDirectOnly` | a nested insert count |
| `check_findings` (verbs) | `CheckSkipsBlocks` | the undefined block inserted inside a block |
| `entity_find_window_and_crossing` (verbs) | `WindowAsCrossing` | the window's matches |

The other rows (`every_kind_is_a_node_keyed_by_handle`, `line_endings_and_padding_are_measured`,
`over_bound_refused`, `a_file_without_an_end_marker_reads`, `units_in_doc_attrs`,
`drawing_info_shape`, `layer_counts`, `text_find`, `header_get`, `check_writeback`,
`verbs_refuse_usage`, `code_pages_and_escapes`, `h1_drawings_read_and_splice`, the kit-session
rows, the model crate's rows, the save-pair judge's row and the fuzz seeds) assert known answers
on our generated drawings. The referee (`cad-measure referee`, with ezdxf as a test checker on a
test machine) compares the pack's reading of the same drawings with ezdxf's.
