# Changelog

## v0.1.0: pairs with kit v0.21.0

The BLEND pack: it reads, indexes and writes BLEND files, and agents write them. **Requires
`k2-mpc-base` v0.21.0** (results as data with their shape named, and a pack's registered
refusals, on the carrier and its pairing), checked out beside this repo (a host vendors both side
by side: `vendor/k2-mpc-base` beside `vendor/k2-mpc-blend`); `Cargo.lock` and `fuzz/Cargo.lock`
name that kit.

### Reading

- **`reader`** reads a file forward only, under `ReadLimits`: the classic header and the newer
  one (17 bytes, 32-byte block headers with 8-byte lengths; any value of it not seen in sample
  files is refused as unknown), plain, gzip or zstd storage, every block to the end block, and
  the file's own schema, through which every struct and field is looked up by name. zstd is read
  frame by frame: each frame's window checked against the reader's limit, which bounds the
  decoder too, dictionaries refused, skippable frames skipped, checksums checked, and each
  block's output bounded by the format before any of it is read. Large blocks are digested as
  they pass, never held; a 1.2 GiB file reads in under 2 MiB of heap. Pointers resolve to the
  one block with their address, else the one owned by the same data-block, else the one
  data-block, else they are ambiguous. Every refusal is typed, with fixed text.
- **`scene`**, the scene model: every data-block with its type code, struct, kind, name,
  library and key (`@ob:Cube`; `@li:lib.blend/ma:Steel` when linked). Scenes hold their
  collections and objects, a camera and a world; objects their data, parent and material
  slots; cameras, lights, materials and collections a few fields, named as the file names them.
  A content digest per data-block, with pointers replaced by what they name, digests another
  save of the same scene the same. For format versions 3.06 to 5.01 (`scene::NAMES_FROM` to
  `NAMES_TO`), visibility bits, object, light and camera types and rotation modes are named,
  learnt from files saved headless at three pinned releases (`PROVENANCE.md`); other versions
  keep them as numbers.
- **`pack`**: `BlendPack`, the kit's `BlobPack`. Each version is indexed into the kit's
  structure index (verbs `object list`, `object show`, `object diff`, `object blame`): scenes,
  then what each holds, then every other data-block but the program's own interface, with the
  file's fields and the model's references as attributes and an object's data as its weak ID.
  `visible` is false for an object or collection disabled in viewports. The index format is 2.

### Writing, merging and the file verbs

- **`write`** patches an object's fixed-size fields in place (location, Euler rotation, scale,
  the three visibility bits, the name inside its field), keeps every other byte, and reads the
  result again before returning it; a compressed file is written back whole (gzip; zstd in
  frames of at most 8 MiB) and decoded again. Refused, typed: format versions outside 3.06 to
  5.01, a key naming no object or several, a linked object, a field not in the schema at the
  expected type and size, an Euler rotation on a quaternion or axis-angle object, a name that is
  empty, holds a NUL, doesn't fit or is taken, a value that isn't finite.
- **`merge`**: the three-way merge of a person's app save with the agent's version, by object
  and field. The save is the file and the agent's field changes are replayed onto it; a field
  both changed is reported (`FieldConflict`), never decided by who saved last. A clash keeps
  both, whole-file: the kit keeps the save as the file and the agent's version as a conflict
  copy in its store. On the blob tier every app save that isn't one side's bytes again keeps
  both (`BlendPack`'s `can_merge` is `false`): the kit hands a pack what the file last held, not
  the version the app had open. `merge_known_base` is the merge for a base known to be the
  app's.
- **`verbs`**, the agent's file verbs: `scene list` (collections, objects, and each camera by
  its object's name), `scene meshes` (the mesh read's report), `object place`,
  `object visibility`, `object rename`, each edit run on the version the agent read and refused
  when the doc has moved on. Each answer comes as data (`verbs::Typed`, `data_spec`
  `blend.file.<verb>/1`). Agents may write (`AgentWrites::Allowed`); a host that wants verbs
  only refuses raw agent writes itself.
- **Limits**: what is decoded whole in memory (the merges, and the file verbs a host runs with
  them) defaults to the live tier's limits, 512 MiB after decompression
  (`BlendPack::default().limits`). The streaming index, which never holds the file, decodes at
  most 4 GiB by default (`BlendPack::index_limits`, `pack::index_read_limits`,
  `INDEX_MAX_RAW_BYTES`); a host sets either.

### The live tier (off by default)

`live::BlendLive`, the kit's live `Pack`, which a host opens a file with only when it chooses to:

- **The store**: each scene's master collection, each collection, and `c0` for objects no
  collection holds, then the objects, with minted keys in file order. An object's transform is
  one field (`xform`, each `f32` as its shortest round-trip text), its visibility three bits,
  its name its file key's, and `edit` says what may be written. Meshes, materials, modifiers and
  every other byte stay in the file the kit keeps (its carrier).
- **Declines**, typed: `live_version` (outside format versions 3.06 to 5.01), `live_ambiguous`,
  `live_value`, `live_too_large` (over `MAX_FILE_BYTES`, 32 MiB stored, or a reader limit), and
  the reader's own.
- **The write-back** patches the changed fields into the carrier with the writer, addressed by
  the carrier's pairing; anything else that differs is refused, never written by guess.
  Untouched, the carrier stays byte for byte. **Outside saves** pair collections and objects by
  file key, else by their data or parent, else mint new keys.
- **The view**: collections are containers, each naming its parent; objects are units, each
  placed in 3D on the kit's view slot (location, rotation in its own mode, scale, parent and
  parent inverse) in the doc's space, with metres per unit only when every scene agrees. Each
  unit carries its mesh's hash and what hides it; each scene marks the one drawn.
- **Verbs**: `scene list`, `collection show`, `object show`, `object place`,
  `object visibility`, `object rename --break-links`. Each returns the kit's result object as
  data (`mpc.result/1`, `data_spec` `blend.<verb>/1`); each refusal prints `refused: <code>`
  and a `next:` verb.

### The mesh read

`meshread::mesh_read` returns a version's scene as K2SCENE1 for a host's renderer, and a report.
It is pure: the same bytes give the same output. Meshes are read by the file's own schema from
any of three layouts and triangulated with their corner order kept; world matrices are composed
from stored transforms; perspective cameras are composed with a fixed arc tangent; each mesh
carries a SHA-256 content hash. Every count, the heap the read needs (512 MiB at most) and the
output's length are checked before anything is allocated. `docs/WIRE.md` is what a client codes
against.

### Refusals

Every refusal of the verbs, the reads and the live tier's open is registered
(`live::errors::ERRORS`, `BLEND-E001` to `BLEND-E023`), with its message, what to do next and,
where there is one, the call to run. A fault of the pack itself (the writer's read-back, the
mesh read's own check) says to report it to the pack's maintainers.

### Tests and tooling

- Files built from a schema of ours (`testgen`) in both header forms, both pointer sizes and byte
  orders, and all three storages; every refusal by type; crafted zstd streams of under 128 KiB
  that would decode to hundreds of MiB, refused in a heap under 8 times the raw cap.
- The merge matrix (88 pairs against an oracle), the live rows, golden view snapshots, each
  verb's result and refusal pinned, the scene's watched turn pinned, and planted bugs each caught
  by its own row (`tests/mutants.rs`, one alone with `BLEND_MUTANT_ONLY`).
- The public corpus: seven CC0 demo files from format versions 2.73 to 5.01, fetched by
  checksum, read, compared with an independent test-only reader, indexed, and drawn and opened
  live where their format version allows.
- Fuzz targets on the reader and the model, the index, the writer and the triangulator.
- `scripts/check.sh`: formatting, lints, the docs with default and with all features, the
  suite and the plants, licences and advisories at the locked versions, the licence files, no
  internal host names, labels or commit hashes in public text, every test target accounted for,
  and the program's headless round trip when a change lands.
- `PROVENANCE.md`: where every layout comes from, with the sample files' licences and checksums.
