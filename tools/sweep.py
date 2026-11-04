#!/usr/bin/env python3
# Test tooling, not shipped. Each mutant removes one check, and the tests must fail. How to run it:
# tools/README.md. The rules, all enforced here:
#  - a pattern that doesn't match the source is STALE, and fails the sweep;
#  - a mutant that doesn't build is BUILD-FAILED, and fails the sweep;
#  - a catch must fail at a line that counts by the written rule in tools/catch.py, at the first
#    failure or at any other failing test: with none, it's NO-ASSERT, and fails the sweep. A script's
#    or the JavaScript checker's own FAILED line is its catch when it names the planted case;
#  - a survivor fails it, unless it is listed in EQUIVALENT with why;
#  - only a mutant listed in EXPECT_ABORT may end in a signal, and that is counted apart.
import subprocess, sys, re, os, importlib.util
sys.dont_write_bytecode=True  # a mutant of a tool and its restore can share a size and a second
os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)),'..'))
_spec=importlib.util.spec_from_file_location('catch','tools/catch.py'); catch=importlib.util.module_from_spec(_spec); _spec.loader.exec_module(catch)
K='vectors/check.mjs'; B='crates/board/src/bad_ids.rs'; R='scripts/check-host-forms.sh'; J='crates/board/src/json.rs'; V='crates/board/src/validate.rs'; M='crates/board/src/model.rs'
Z='crates/board/src/zip.rs'; P='crates/board/src/package.rs'; L='crates/board/src/limits.rs'
A='crates/board/src/parts.rs'; C='crates/board/src/conformance.rs'; S='crates/board/src/schema.rs'; O='crates/board/src/ops.rs'
F='false'
MUT=[
# json
('json-dup',J,'if out.contains_key(&key) {','if false {'),
('json-depth',J,'if self.depth > self.limits.depth {','if false {'),
('json-values',J,'if self.values > self.limits.values {','if false {'),
('json-after-doc',J,'if p.i != p.b.len() {','if false {'),
('json-low-surrogate',J,'if !(0xDC00..=0xDFFF).contains(&low) {','if false {'),
('json-control-in-string',J,"if c == b'\"' || c == b'\\\\' || c < 0x20 {","if c == b'\"' || c == b'\\\\' {"),
('json-leading-zero',J,"Some(b'0') => self.i += 1,","Some(b'0') => self.digits(),"),
('json-sort-utf8',J,'keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));','keys.sort();'),
('json-escape-control',J,'} else if c < 0x20 {','} else if false {'),
('json-escape-upper',J,'\\\\u{c:04x}','\\\\u{c:04X}'),
('json-canonical-depth',J,'if depth > Limits::V1.depth {','if false {'),
# limits
('limit-ceiling',L,'if ours > ceiling {','if false {'),
('limit-depth-raised',L,'depth: 64,','depth: 65,'),
('header-bound',Z,'if h + CENTRAL_LEN > b.len() {','if false {'),
('noop-added',O,'self.added.is_empty()','true'),
('noop-changed',O,'&& self.changed.is_empty()',''),
('noop-deleted',O,'&& self.deleted.is_empty()',''),
('noop-settings',O,'&& !self.settings_changed',''),
('noop-people',O,'&& self.people_added.is_empty()',''),
('noop-blobs',O,'&& self.blobs_cleared.is_empty()',''),
('bad-names-filled-open',M,'Ok(again) => bad_ids::name_them(r, &again),','Ok(_) => r,'),
('bad-names-filled-validate',M,'bad_ids::name_them(r, &self.to_value())','r'),
('bad-cap',B,'.take(BAD_IDS_MAX)',''),
('bad-total-at-cap',B,'r.bad_ids_total = found.order.len() as u64;','r.bad_ids_total = found.order.len().min(BAD_IDS_MAX) as u64;'),
('bad-truncated',B,'r.bad_ids_truncated = found.order.len() > BAD_IDS_MAX;','r.bad_ids_truncated = false;'),
('bad-truncated-at-cap',B,'found.order.len() > BAD_IDS_MAX;','found.order.len() >= BAD_IDS_MAX;'),
('bad-once-each',B,'if Id::new(id).is_err() && self.seen.insert(id) {','if Id::new(id).is_err() {'),
('bad-once-each-cut',B,'&& self.seen.insert(id) {','&& self.seen.insert(id.get(..64).unwrap_or(id)) {'),
('bad-order-sorted',B,'r.bad_ids_total = found.order.len() as u64;','found.order.sort(); r.bad_ids_total = found.order.len() as u64;'),
('bad-cut',B,'if id.chars().count() <= BAD_ID_MAX_CHARS {','if true {'),
('bad-cut-mark',B,"cut.push('\\u{2026}');",''),
('bad-code-only',B,'if r.code != RefusalCode::IdInvalid {','if false {'),
('bad-frames',B,'found.value(Some(f));',''),
('bad-people',B,'found.keys(people);','let _ = people;'),
('bad-object-id',B,'self.value(o.get("id"));',''),
('bad-refs',B,'Kind::ObjectRef | Kind::GroupId => self.value(Some(v)),',''),
('bad-end',B,'self.value(end.get("id"));','let _ = end;'),
('bad-items',B,'Kind::Checks | Kind::Columns => self.item_ids(v),',''),
('bad-cells',B,'self.keys(cells);','let _ = cells;'),
('bad-row-id',B,'self.value(row.get("id"));',''),
('bad-reply-by',B,'self.value(reply.get("by"));',''),
('bad-reply-id',B,'self.value(reply.get("id"));',''),
('bad-reactions',B,'self.value(Some(token));','let _ = token;'),
('bad-votes',B,'self.keys(votes);','let _ = votes;'),
('bad-created',B,'self.value(created.get("by"));','let _ = created;'),
('bad-unknown-fields',B,'} else if let Some(spec) = schema::field_of(type_name, name) {','} else if let Some(spec) = schema::field_of("mind", "parent").filter(|_| type_name != "z" && name == "later") {'),
# validate
('name-grammar',V,"matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-')",'{ let _ = c; true }'),
('name-length',V,'(1..=schema::NAME_MAX_BYTES).contains(&s.len())','!s.is_empty()'),
('colour-hex',V,'return hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit());','return hex.len() == 6;'),
('colour-len',V,'return hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit());','return hex.bytes().all(|c| c.is_ascii_hexdigit());'),
('chars-utf16',V,'s.chars().count()','s.encode_utf16().count()'),
('field-name-length',V,'let ok = (1..=schema::FIELD_NAME_MAX_BYTES).contains(&name.len())','let ok = true'),
('field-name-control',V,'&& !name.chars().any(|c| (c as u32) < 0x20);',';'),
('value-numbers',V,'check_value(v, &f_at, 2)?;',''),
('value-extra',V,'check_value(v, &format!("/{name}"), 2)?;',''),
('value-depth',V,'if depth > Limits::V1.depth {','if false {'),
('person-known',V,'if self.people.contains_key(&id) {','if true {'),
('list-limit',V,'if a.len() > max {','if false {'),
('sub-id-twice',V,'if !seen.insert(id.clone()) {','if false {'),
('text-limit',V,'if chars(s) > max {','if false {'),
('time-long',V,'if b.len() > schema::TIMESTAMP_MAX_BYTES || b.len() < 20 {','if b.len() < 20 {'),
('time-shape',V,'if !shape {','if false {'),
('time-zone',V,'if !zone_ok {','if false {'),
('time-range',V,'if !in_range {','if false {'),
('time-fraction',V,'if i == start {','if false {'),
('end-form',V,'_ => Err(value_err(at, "must be {id, anchor?, hint?} or {x, y}")),','_ => Ok(()),'),
('end-anchor',V,'if !is_name(as_str(a, &a_at)?) {','if false {'),
('end-hint',V,'point(as_obj(h, &h_at, "a point")?, &h_at)?;',''),
('open-name',V,'if !is_name(as_str(v, at)?) {','if false {'),
('colour-check',V,'if !is_color(as_str(v, at)?) {','if false {'),
('point-pair',V,'if xy.len() != 2 {','if false {'),
('check-done',V,'if !matches!(required(f, "done", &c_at)?, Value::Bool(_)) {','if false {'),
('column-width',V,'if as_num(w, &w_at)? < 0.0 {','if false {'),
('reaction-key',V,'if !key_ok {','if false {'),
('reaction-order',V,'if last.is_some_and(|l| l >= t) {','if false {'),
('reaction-count',V,'if f.len() > schema::REACTIONS_MAX {','if false {'),
('votes-count',V,'if f.len() > schema::VOTERS_MAX {','if false {'),
('votes-whole',V,'if n.fract() != 0.0 || !(1.0..=schema::VOTES_MAX).contains(&n) {','if false {'),
('crop-unit',V,'let in_unit = |n: f64| (0.0..=1.0).contains(&n);','let in_unit = |_: f64| true;'),
('crop-min-w',V,'|| w < schema::CROP_MIN',''),
('crop-min-h',V,'|| h < schema::CROP_MIN',''),
('crop-edge-x',V,'|| x + w > schema::CROP_EDGE',''),
('crop-edge-y',V,'|| y + h > schema::CROP_EDGE',''),
('link-length',V,'if chars(s) > schema::LINK_MAX_CHARS {','if false {'),
('link-scheme',V,'let rest = if lower.starts_with("https://") {','let rest = if true {'),
('link-rest',V,"if rest.is_empty() || rest.chars().any(|c| c.is_whitespace() || c == '\\u{FEFF}') {",'if false {'),
('blob-members',V,'if f.len() != 1 {','if false {'),
('blob-hash',V,'Sha256::parse(as_str(required(f, "blob", at)?, &b_at)?)','Sha256::parse("sha256-0000000000000000000000000000000000000000000000000000000000000000")'),
('voting-active',V,'if !matches!(required(f, "active", at)?, Value::Bool(_)) {','if false {'),
('voting-per',V,'if n.fract() != 0.0 || n < 1.0 {','if false {'),
('voting-per-from-one',V,'if n.fract() != 0.0 || n < 1.0 {','if n.fract() != 0.0 {'),
('voting-per-whole',V,'if n.fract() != 0.0 || n < 1.0 {','if n < 1.0 {'),
('pair-refused-accepted',C,'Ok(_) => Err(format!("wanted refusal {code}, but it was accepted")),','Ok(_) => Ok(()),'),
('pair-refused-code',C,'Err(r) if r.code.as_str() == code => Ok(()),','Err(_) => Ok(()),'),
('pair-direction',C,'if ["both", "widget_to_file", "file_to_widget"].contains(&s.as_str()))','if !s.is_empty())'),
('cell-column',V,'if !columns.contains(col.as_str()) {','if false {'),
('frames-twice',V,'if listed.len() != b.frames.len() || listed != frame_objects {','if listed != frame_objects {'),
('frames-set',V,'if listed.len() != b.frames.len() || listed != frame_objects {','if listed.len() != b.frames.len() {'),
('object-twice',V,'if !ids.insert(&o.id) {','let _ = ids.insert(&o.id); if false {'),
('person-name-limit',V,'if chars(name) > schema::PERSON_NAME_MAX_CHARS {','if false {'),
('person-kind',V,'if !is_name(kind) {','if false {'),
('type-in-memory',V,'if !is_name(t) {','if false {'),
# model
('id-chars',M,".all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'z' | b'.' | b'_' | b'-'));",'.all(|_| true);'),
('id-long',M,'let ok = (1..=ID_MAX_BYTES).contains(&s.len())','let ok = !s.is_empty()'),
('id-empty',M,'let ok = (1..=ID_MAX_BYTES).contains(&s.len())','let ok = (0..=ID_MAX_BYTES).contains(&s.len())'),
('type-grammar',M,'if !validate::is_name(s) {','if false {'),
('objects-limit',M,'if objects as u64 > limits.objects {','if false {'),
('people-limit',M,'if people as u64 > limits.people {','if false {'),
('ref-frame-type',M,'Some(t) if t.kind != ObjectType::Frame => RefState::Suspect,',''),
('ref-mind-loop',M,'Some(_) if mind_cut.contains(&o.id) => RefState::Suspect,',''),
('ref-frame-loop',M,'Some(_) if frame_cut.contains(&o.id) => RefState::Suspect,',''),
('ref-loop-cut',M,'if let Some(smallest) = seen.iter().min() {','if let Some(smallest) = seen.iter().max() {'),
('ref-hint',M,'Some((*x, *y))','{ let _ = (x, y); None }'),
('blob-ref-lone',M,'if f.len() == 1 {','if true {'),
('blob-ref-arrays',M,'Value::Arr(a) => a.iter().for_each(|x| walk(x, out)),','Value::Arr(_) => {}'),
('strip-names',M,'p.name = None;','let _ = &p;'),
('token-form',M,'Id(format!("p-{}", hex(&random)))','Id(format!("p{}", hex(&random)))'),
('board-ceiling',M,'limits.check_ceiling()?;','',0),
('validate-ceiling',M,'limits.check_ceiling()?;','',1),
('rev-hash',M,'Ok(Sha256::of(&self.canonical_json()?))','Ok(Sha256::of(b""))'),
('rev-settings-hash',M,'self.settings.clone(),','Fields::new(),'),
('person-kind-parse',M,'Some(Value::Str(s)) => Some(s),','Some(Value::Str(_)) => None,',1),
('person-kind-out',M,'f.insert("kind".into(), Value::Str(kind.clone()));','let _ = kind;'),
# zip
('trailing-is-stray',Z,'Refusal::new(C::ZipStrayBytes, "bytes after the end record")','no_end()'),
('z64-counts',Z,'here == u16::MAX || total == u16::MAX || ',''),
('z64-size',Z,'central_size == u32::MAX || ',''),
('z64-at',Z,'|| central_at == u32::MAX',''),
('multi-disk',Z,'if disk != 0 || central_disk != 0 || here != total {','if false {'),
('entries-limit',Z,'if u64::from(total) > l.entries {','if false {'),
('central-past-end',Z,'if central_end > end_at {','if false {'),
('central-then-stray',Z,'if central_end < end_at {','if false {'),
('z64-locator',Z,'if le32(&sig, 0) == SIG_ZIP64_LOCATOR {','if false {'),
('central-cap',Z,'if central_size > CENTRAL_MAX {','if false {'),
('dup-names',Z,'if !seen.insert(name.to_ascii_lowercase()) {','let _ = seen.insert(name.to_ascii_lowercase()); if false {'),
('dup-fold',Z,'if !seen.insert(name.to_ascii_lowercase()) {','if !seen.insert(name.clone()) {'),
('part-limit',Z,'if c.entry.usize_ > max {','if false {'),
('total-limit',Z,'if total_bytes > l.total_bytes {','if false {'),
('ratio-declared',Z,'if !ratio_ok(c.entry.usize_, c.entry.csize, l) {','if false {'),
('central-count',Z,'if p != central.len() {','if false {'),
('central-sig',Z,'if le32(b, h) != SIG_CENTRAL {','if false {'),
('encrypted',Z,'if flags & FLAGS_ENCRYPTED != 0 {','if false {'),
('flags',Z,'if flags & !FLAGS_OK != 0 {','if false {'),
('z64-entry',Z,'if csize == u32::MAX || usize_ == u32::MAX || local_offset == u32::MAX {','if false {'),
('extra-central',Z,'check_extra(extra, &printable)?;',''),
('disk-start',Z,'if disk_start != 0 {','if false {'),
('method',Z,'if method != METHOD_STORE && method != METHOD_DEFLATE {','if false {'),
('entry-comment',Z,'if comment_len != 0 {','if false {'),
('regular-unix',Z,'let not_regular = (made_by_host == 3 && unix_type != 0 && unix_type != REGULAR)','let not_regular = false'),
('regular-dir',Z,'|| external & 0x10 != 0',''),
('regular-reparse',Z,'|| external & 0x400 != 0;',';'),
('name-check',Z,'Ok(n) if name_ok(n) => n.to_string(),','Ok(n) => n.to_string(),'),
('store-sizes',Z,'if method == METHOD_STORE && csize != usize_ {','if false {'),
('z64-extra-id',Z,'if id == 1 {','if false {'),
('extra-past-end',Z,'if p > extra.len() {','if false {'),
('local-sig',Z,'if le32(&h, 0) != SIG_LOCAL {','if false {'),
('local-flags',Z,'let same = le16(&h, 6) == c.flags','let same = true'),
('local-method',Z,'&& le16(&h, 8) == c.entry.method',''),
('local-crc',Z,'&& le32(&h, 14) == c.entry.crc',''),
('local-csize',Z,'&& u64::from(le32(&h, 18)) == c.entry.csize',''),
('local-usize',Z,'&& u64::from(le32(&h, 22)) == c.entry.usize_',''),
('local-name',Z,'&& tail[..name_len] == c.raw_name[..];',';'),
('extra-local',Z,'check_extra(&tail[name_len..], &name)?;',''),
('layout-overlap',Z,'if c.local_offset < expected {','if false {'),
('layout-gap',Z,'if c.local_offset > expected {','if false {'),
('layout-into-central',Z,'if expected > central_at {','if false {'),
('layout-before-central',Z,'if expected < central_at {','if false {'),
('crc',Z,'if !crc != e.crc {','if false {'),
('inflate-stop',Z,'if written + r.bytes_written as u64 > declared {','if false {'),
('ratio-stream',Z,'if !ratio_ok(written, consumed, l) {','if false {'),
('inflate-short',Z,'if written != declared || consumed != csize {','if consumed != csize {'),
('inflate-pad',Z,'if written != declared || consumed != csize {','if written != declared {'),
('ratio-rule',Z,'out <= l.ratio_floor || out <= consumed.saturating_mul(l.ratio)','true'),
('name-total',Z,'if name.len() > NAME_MAX_BYTES {','if false {'),
('name-segments',Z,'if segments.len() > SEGMENTS_MAX {','if false {'),
('name-segment-empty',Z,'!seg.is_empty()','true'),
('name-segment-len',Z,'&& seg.len() <= SEGMENT_MAX_BYTES',''),
('name-start-dot',Z,"&& !seg.starts_with('.')",''),
('name-end-dot',Z,"&& !seg.ends_with('.')",''),
('name-chars',Z,"c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')","true"),
('name-reserved',Z,'&& !windows_reserved(seg)',''),
('reserved-device',Z,'"con" | "prn" | "aux" | "nul" => true,','"zzz" => true,'),
('reserved-com',Z,'(stem.starts_with("com") || stem.starts_with("lpt"))','stem.starts_with("lpt")'),
('reserved-lpt',Z,'(stem.starts_with("com") || stem.starts_with("lpt"))','stem.starts_with("com")'),
('writer-ratio',Z,'if check.is_err() {','if false {'),
('crc-table',Z,'^ CRC_TABLES[0][(hi >> 24) as usize];','^ CRC_TABLES[1][(hi >> 24) as usize];'),
# package
('format',P,'Some(Value::Str(f)) if f == FORMAT => {}','Some(Value::Str(_)) => {}'),
('min-reader',P,'if min_reader > MIN_READER {','if false {'),
('whole-zero',P,'(1.0..=f64::from(u32::MAX)).contains(&n)','(0.0..=f64::from(u32::MAX)).contains(&n)'),
('parts-name',P,'if !zip::name_ok(name) {','if false {'),
('parts-twice',P,'if !seen.insert(name.to_ascii_lowercase()) {','let _ = seen.insert(name.to_ascii_lowercase()); if false {'),
('parts-fold',P,'if !seen.insert(name.to_ascii_lowercase()) {','if !seen.insert(name.to_string()) {'),
('parts-media',P,'if !media_type_ok(&p.media_type) {','if false {'),
('parts-role',P,'if !role_ok(&p.role) {','if false {'),
('parts-self',P,'if name == NAME_MANIFEST {','if false {'),
('parts-board-role',P,'if (name == NAME_BOARD) != (p.role == ROLE_BOARD) {','if false {'),
('parts-preview-role',P,'if (name == NAME_PREVIEW) != (p.role == ROLE_PREVIEW) {','if false {'),
('parts-blob-role',P,'if name.starts_with("blobs/") != (p.role == ROLE_BLOB) {','if false {'),
('parts-blob-name',P,'if p.role == ROLE_BLOB && p.sha256.blob_name() != name {','if false {'),
('parts-one-board',P,'if boards != 1 {','if false {'),
('refs-present',P,'.filter(|h| !names.contains(h.blob_name().as_str()))','.filter(|_| false)'),
('manifest-central-first',P,'if first.name != NAME_MANIFEST || lowest.map(|e| e.name.as_str()) != Some(NAME_MANIFEST) {','if lowest.map(|e| e.name.as_str()) != Some(NAME_MANIFEST) {'),
('manifest-file-first',P,'if first.name != NAME_MANIFEST || lowest.map(|e| e.name.as_str()) != Some(NAME_MANIFEST) {','if first.name != NAME_MANIFEST {'),
('part-size',P,'if e.usize_ != p.bytes {','if false {'),
('part-unlisted',P,'if name != NAME_MANIFEST && !manifest.parts.iter().any(|p| &p.name == name) {','if false {'),
('board-hash',P,'if board_hash != part.sha256 {','if false {'),
('open-missing',P,'self.missing.clone()','Vec::new()'),
('read-hash',P,'if Sha256::of(&bytes) != part.sha256 {','if false {'),
('guard-board',P,'let guard = Sha256::of(&manifest.to_canonical_json()?);','let guard = board_hash;'),
('guard-unsorted',P,'parts.sort_by(|a, b| a.name.cmp(&b.name));',''),
('write-validate',P,'board.validate(l)?;',''),
('write-reserved',P,'if meta.extra.contains_key(reserved) {','if false {'),
('write-parts',P,'check_parts(&all)?;',''),
('write-refs',P,'if let Some(h) = missing_from(board, &all).first() {','if let Some(h) = None::<&Sha256> {'),
('version-min-reader',P,'let min_reader = meta.min_reader.max(MIN_READER);','let min_reader = MIN_READER;'),
('version-kept',P,'version: meta.version.max(VERSION).max(min_reader),','version: VERSION.max(min_reader),'),
('version-floor',P,'version: meta.version.max(VERSION).max(min_reader),','version: meta.version.max(min_reader),'),
('min-reader-floor',P,'let min_reader = meta.min_reader.max(MIN_READER);','let min_reader = meta.min_reader;'),
('meta-version',P,'version: self.version,','version: VERSION,'),
('write-limits',P,'check_write_limits(&all, l)?;',''),
('write-manifest-limit',P,'if manifest_json.len() as u64 > l.manifest_bytes {','if false {'),
('write-size',P,'if bytes.len() as u64 != p.bytes {','if false {'),
('write-hash',P,'if Sha256::of(&bytes) != p.sha256 {','if false {'),
('write-sorted',P,'listed.sort_by(|a, b| a.name.cmp(&b.name));',''),
('write-entries',P,'if all.len() as u64 + 1 > l.entries {','if false {'),
('write-part-max',P,'if p.bytes > max {','if false {'),
('write-total',P,'if total > l.total_bytes {','if false {'),
('meta-base',P,'base: self.base.clone(),','base: None,'),
('meta-extra',P,'extra: self.extra.clone(),','extra: Fields::new(),'),
('guard-of-ceiling',P,'l.check_ceiling()?;','',0),
('open-ceiling',P,'l.check_ceiling()?;','',1),
# parts
('overlay-removed-list',A,'!self.removed.contains(&p.name) && ',''),
('overlay-removed-read',A,'if self.removed.contains(name) {','if false {'),
('overlay-re-add',A,'self.removed.remove(&h.blob_name());',''),
('overlay-added-first',A,'if self.added.list().iter().any(|p| p.name == name) {','if false {'),
('overlay-preview-replaces',A,'self.removed.remove("preview.png");',''),
# runner
('runner-code',C,'if r.code.as_str() == want {','if true {'),
('runner-canonical',C,'if o.canonical != want {','if false {'),
('runner-sha',C,'if got != hash {','if false {'),
('runner-guard',C,'if got != guard {','if false {'),
('runner-stable',C,'&& w.package_guard != guard','&& false'),
('runner-missing',C,'if got_missing != want_missing {','if false {'),

# ops
('group-partial',O,'stage.op(op, group.by.as_ref())?;','if let Err(c) = stage.op(op, group.by.as_ref()) { let plan = stage.into_plan(); plan.commit(board); return Err(c); }'),
('group-partial-check',O,'stage.check()?;','if let Err(c) = stage.check() { let plan = stage.into_plan(); plan.commit(board); return Err(c); }'),
('rev-object',O,'if start.rev().map_err(refused)? != *rev {','if false {'),
('rev-added',O,'if self.added.contains(id) {','if false {'),
('rev-settings',O,'if self.board.settings_rev().map_err(refused)? != *rev {','if false {'),
('add-exists',O,'if let Some(there) = self.current(&object.id) {','if let Some(there) = None::<&Object> {'),
('person-same',O,'if there == person {','if true {'),
('del-gone-report',O,'self.report.already_gone.push(id.clone());',''),
('lock-set',O,'if !free_set || !free_unset {','if false {'),
('lock-free-false',O,'(k != "locked" || *v == Value::Bool(false))','true'),
('lock-free-list',O,'const LOCK_FREE: [&str; 3] = ["locked", "votes", "reactions"];','const LOCK_FREE: [&str; 1] = ["locked"];'),
('lock-unset',O,'let free_unset = unset.iter().all(|k| LOCK_FREE.contains(&k.as_str()));','let free_unset = true;'),
('lock-del',O,'if is_locked(current) {\n            return Err(locked(id));\n        }\n        self.touched.insert(id.clone(), None);','let _ = current;\n        self.touched.insert(id.clone(), None);'),
('lock-z',O,'if is_locked(o) {','if false {',0),
('lock-frame',O,'if is_locked(o) {','if false {',1),
('lock-state-start',O,'if is_locked(current) {\n            // The same rule','if self.start(id).is_some_and(is_locked) {\n            // The same rule'),
('reserved-names',O,'if name == "id" || name == "type" {','if false {'),
('merge-reactions',O,'("reactions", Some(token)) => merge_reactions(fields.get(name), value, token),',''),
('merge-votes',O,'("votes", Some(token)) => merge_votes(fields.get(name), value, token),',''),
('merge-lists',O,'_ if merges_by_id(name) => Some(merge_by_id(fields.get(name), value)),',''),
('merge-no-columns',O,'"checks" | "replies" | "rows" | "columns"','"checks" | "replies" | "rows"'),
('merge-no-rows',O,'"checks" | "replies" | "rows" | "columns"','"checks" | "replies" | "columns"'),
('merge-no-replies',O,'"checks" | "replies" | "rows" | "columns"','"checks" | "rows" | "columns"'),
('merge-no-checks',O,'"checks" | "replies" | "rows" | "columns"','"replies" | "rows" | "columns"'),
('merge-drops-sub',O,'merged.extend(f.iter().map(|(k, v)| (k.clone(), v.clone())));','merged = f.clone();'),
('reaction-others',O,'Value::Str(s) if s != token.as_str() => Some(s.clone()),','Value::Str(_) => None,'),
('reaction-mine-stays',O,'Value::Str(s) if s != token.as_str() => Some(s.clone()),','Value::Str(s) => Some(s.clone()),'),
('reaction-empty-key',O,'.filter(|(_, people)| !people.is_empty())',''),
('reaction-empty-field',O,'if result.is_empty() {','if false {'),
('votes-others',O,'Some(Value::Obj(old)) => old.clone(),','Some(Value::Obj(_)) => Fields::new(),'),
('votes-mine-stays',O,'out.remove(token.as_str());',''),
('votes-empty-field',O,'if out.is_empty() {','if false {'),
('unset-caller',O,'if by.is_some() && (name == "reactions" || name == "votes") {','if false {'),
('clear-known',O,'o.fields.remove(name);',''),
('clear-touched',O,'cleared |= clear_object(state, &text);','let _ = &state;'),
('clear-settings',O,'if clear_fields(&mut settings, &text) {','if false {'),
('clear-extra',O,'if clear_fields(&mut extra, &text) {','if false {'),
('clear-people',O,'people_cleared |= clear_fields(&mut p.extra, &text);','let _ = &p;'),
('clear-nested',O,'any |= clear_value(x, text);','let _ = &x;',1),
('clear-report',O,'self.report.blobs_cleared.push(*hash);',''),
('check-objects',O,'for o in self.touched.values().flatten() {','for o in self.touched.values().flatten().take(0) {'),
('check-settings',O,'validate::check_settings(settings, people).map_err(refused)?;','let _ = settings;'),
('check-person',O,'validate::check_person(token, p).map_err(refused)?;','let _ = (token, p);'),
('check-object-count',O,'if count > Limits::V1.objects as i64 {','if false {'),
('check-people-count',O,'if people.len() as u64 > Limits::V1.people {','if false {'),
('frame-kind',O,'if o.kind != ObjectType::Frame {','if false {'),
('add-joins-frames',O,'if kind == ObjectType::Frame {','if false {'),
('del-leaves-frames',O,'board.frames.retain(|f| *f != id);',''),
('place-front',O,'Position::Front => (list.len(), true),','Position::Front => (0, true),'),
('place-back',O,'Position::Back => (0, true),','Position::Back => (list.len(), true),'),
('place-after',O,'return (at + 1, i == 0);','return (at, i == 0);'),
('place-exact',O,'return (at + 1, i == 0);','return (at + 1, true);'),
('place-self',O,'names.iter().filter(|n| *n != id).enumerate()','names.iter().enumerate()'),
('place-none',O,'(0, false)','(0, true)'),
('move-noop',O,'if at == from {','if false {'),
('set-noop',O,'} else if self.index.get(id.as_str()).map(|i| &self.board.objects[*i]) != Some(&next) {','} else {'),
('settings-noop',O,'.is_some_and(|s| *s != self.board.settings)','.is_some()'),
('noop-moved',O,'&& self.moved.is_empty()',''),
# drift
('drift-schema-table',S,'f("text", Kind::Str(5000), text("")),','f("text", Kind::Str(6000), text("")),',0),
('drift-limit',L,'objects: 100_000,','objects: 100_001,'),
('drift-schema-file','schema/board-schema.json','"format":"k5board"','"format":"k9board"'),
('drift-spec-table','docs/FORMAT.md','| `objects` | 100000 |','| `objects` | 100001 |'),
('drift-vector-file','vectors/json/empty.json','"frames"','"frameZ"'),
('drift-index-expectation','vectors/index.json','"refusal":"zip_crc"','"refusal":"zip_method"',0),
]
os.environ['CARGO_TERM_COLOR']='never'
NUM_RUST=[
('num-digits',J,'if digits.len() > MAX_SIGNIFICANT_DIGITS {','if false {'),
('num-digits-18',J,'pub const MAX_SIGNIFICANT_DIGITS: usize = 17;','pub const MAX_SIGNIFICANT_DIGITS: usize = 18;'),
('num-digits-16',J,'pub const MAX_SIGNIFICANT_DIGITS: usize = 17;','pub const MAX_SIGNIFICANT_DIGITS: usize = 16;'),
('num-trailing-zeros-count',J,"let digits = kept.trim_end_matches('0').to_string();",'let digits = kept.to_string();'),
('num-leading-zeros-count',J,"let kept = joined.trim_start_matches('0');",'let kept = joined.as_str();'),
('num-magnitude',J,'if past_max(&digits, exp) {','if false {'),
('num-magnitude-on-double',J,'if past_max(&digits, exp) {','if lit.parse::<f64>().unwrap_or(f64::INFINITY).abs() > MAX_NUMBER {'),
('num-underflow',J,'if v == 0.0 {\n        return Err(inexact(','if false {\n        return Err(inexact('),
('num-zero-early',J,'if digits.is_empty() {\n        return Ok(0.0);','if false {\n        return Ok(0.0);'),
('num-past-equal',J,'std::cmp::Ordering::Equal => digits > MAX_DIGITS,','std::cmp::Ordering::Equal => digits >= MAX_DIGITS,'),
('num-past-greater',J,'std::cmp::Ordering::Greater => true,','std::cmp::Ordering::Greater => false,'),
('num-past-less',J,'std::cmp::Ordering::Less => false,','std::cmp::Ordering::Less => true,'),
('num-exp-held',J,"let held = if text.starts_with('-') { -HUGE } else { HUGE };",'let held = 0;'),
('num-exp-leading',J,'(digits, exp + int.len() as i64 - leading)','(digits, exp + int.len() as i64)'),
('num-exp-int',J,'(digits, exp + int.len() as i64 - leading)','(digits, exp - leading)'),
('num-literal-length',J,'if lit.len() > MAX_NUMBER_LITERAL {','if false {'),
('num-print-range',J,'if !v.is_finite() || v.abs() > MAX_NUMBER {','if false {'),
('num-print-small',J,'} else if -6 < n && n <= 0 {','} else if -7 < n && n <= 0 {'),
('num-print-zero',J,'return Ok("0".to_string());','return Ok(format!("{v}"));'),
('num-print-magnitude',J,'if !v.is_finite() || v.abs() > MAX_NUMBER {','if !v.is_finite() {'),
('num-print-finite',J,'if !v.is_finite() || v.abs() > MAX_NUMBER {','if v.abs() > MAX_NUMBER {'),
('num-negative-zero',J,'return Ok(0.0);','return Ok(lit.parse().unwrap_or(0.0));'),
]
NUM_JS=[
('js-fidelity-skip',K," && c.layer !== 'fidelity') { skipped++",") { skipped++"),
('js-bad-once-each-cut',K,"if (typeof v === 'string' && !ID.test(v)) seen.add(v)","if (typeof v === 'string' && !ID.test(v) && ![...seen].some((x) => x.slice(0, 64) === v.slice(0, 64))) seen.add(v)"),
('js-bad-cap',K,'ids: all.slice(0, 1000).map(shown)','ids: all.map(shown)'),
('js-bad-total',K,'total: all.length,','total: Math.min(all.length, 1000),'),
('js-bad-truncated',K,'truncated: all.length > 1000','truncated: false'),
('js-bad-cut',K,'c.length > 64 ?','false ?'),
('js-bad-people',K,'if (board.people) sorted(board.people).forEach(note)',''),

('js-digits',K,'if (digits.length > MAX_DIGITS) throw','if (false) throw'),
('js-digits-18',K,'const MAX_DIGITS = 17','const MAX_DIGITS = 18'),
('js-trailing-zeros-count',K,"const digits = kept.replace(/0+$/, '')",'const digits = kept'),
('js-magnitude',K,"if (exp > 16n || (exp === 16n && digits > '9007199254740992')) {",'if (false) {'),
('js-magnitude-on-double',K,"if (exp > 16n || (exp === 16n && digits > '9007199254740992')) {",'if (Math.abs(Number(lit)) > 2 ** 53) {'),
('js-underflow',K,"if (v === 0) throw new Refused('json_number_inexact')",''),
('js-equal',K,"digits > '9007199254740992'","digits >= '9007199254740992'"),
]

PROP=[
('prop-central-bound',Z,'if next > b.len() {','if false {'),
('prop-header-bound',Z,'if h + CENTRAL_LEN > b.len() {','if false {'),
('prop-extra-bound',Z,'if p + 4 > extra.len() {','if false {'),
('prop-depth',J,'if self.depth > self.limits.depth {','if false {'),
]
SCRIPT=[
('licences-fuzz-list','scripts/check-licences.sh','allowed = ALLOWED | {"NCSA"} if where == "fuzz" and p["name"] == "libfuzzer-sys" else ALLOWED','allowed = ALLOWED | {"NCSA"}'),
('licences-fuzz-name','scripts/check-licences.sh','if where == "fuzz" and p["name"] == "libfuzzer-sys" else ALLOWED','if where == "fuzz" else ALLOWED'),
('licences-fuzz-where','scripts/check-licences.sh','if where == "fuzz" and p["name"] == "libfuzzer-sys" else ALLOWED','if p["name"] == "libfuzzer-sys" else ALLOWED'),
('licences-off-list','scripts/check-licences.sh','if not ok(lic, allowed):','if False:'),
('forms-blob','scripts/check-host-forms.sh',"PATTERNS='k2"+"blob|","PATTERNS='zzz|"),
('forms-board','scripts/check-host-forms.sh',"|k2-"+"board|",'|zzz|'),
('forms-person','scripts/check-host-forms.sh',"|person:[a-z0-9]|",'|zzz|'),
('forms-agent','scripts/check-host-forms.sh',"|agent:[a-z0-9]'","|zzz'"),
('forms-empty-tree','scripts/check-host-forms.sh','[ "$FILES" -gt 0 ] ||','true ||'),
# the catch rule itself, against its self-test
('catch-plain-expect','tools/catch.py',"    return None, 'expect'","    return 'expect', ''"),
('catch-library','tools/catch.py',"        if start is None or line < start:\n            return None, 'library'","        if False:\n            return None, 'library'"),
('catch-unit-tests','tools/catch.py',"        start = test_module(lines)","        start = None"),
('catch-not-test-code','tools/catch.py',"    elif not path.startswith(TESTS):\n        return None, 'not test code'","    elif False:\n        return None, 'not test code'"),
('catch-script-bare','tools/catch.py',"m = re.match(r'FAILED ([^:\\s][^:]*): \\S', line)","m = re.match(r'FAILED ()', line)"),
('catch-non-literal','tools/catch.py','\\(\\s*(r#*)?"(.*)',"\\(\\s*(r#*)?\"?(.*)"),
('catch-support-panic','tools/catch.py',"    if path.startswith(SUPPORT):\n        return None, 'support'","    if False:\n        return None, 'support'"),
('credit-tree-header','crates/board/src/verbs/templates.rs','Copyright (c) 2026 Baden Ballard, MIT\n//! (see','\n//! (see'),
('credit-tree-generated','docs/PARITY.md',"The list of actions is Board's own: portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT; see `THIRD-PARTY-NOTICES.md`.",''),
('credit-self-header','scripts/check-credit.sh','elif CREDIT not in header(text):','elif CREDIT not in flat(text):'),
('credit-self-marker','scripts/check-credit.sh',"MARKER = re.compile(r'\\b(ports?|ported|derived?|derives|comes?|taken|adapted|copied)\\s+from\\s+Board\\b', re.I)","MARKER = re.compile(r'Ported from Board')"),
('credit-self-mit','scripts/check-credit.sh',"        found.append(('THIRD-PARTY-NOTICES.md', f'the MIT notice lacks \"{line[:40]}\"'))","        pass"),
('catch-literal-below','tools/catch.py',"text = lines[line - 1][col - 1:] + '\\n' + '\\n'.join(lines[line:line + 3])","text = lines[line - 1][col - 1:]"),
]
EQUIVALENT={
'central-past-end':'read_at then fails on the same bytes with the same zip_malformed; the check only avoids allocating first',
'version-floor':'VERSION and MIN_READER are both 1, so min_reader already holds the floor until they differ',
}
EXPECT_ABORT={'prop-depth':'with the bound gone, 5,000,000 open brackets end the process on its stack; limit_depth itself is held at an assert by json-depth and limit-depth-raised'}
MG='crates/board/src/merge.rs'
F='crates/board/src/file.rs'; R='crates/board/src/verbs/read.rs'; E='crates/board/src/verbs/edit.rs'; G='crates/board/src/verbs/mod.rs'
A='crates/board/src/verbs/arrange.rs'; N='crates/board/src/verbs/clone.rs'; T='crates/board/src/verbs/templates.rs'; W='crates/board/src/verbs/raw.rs'; I='crates/board/src/verbs/inverse.rs'
VERB=[
# file
('file-guard',F,'if now != read {','if false {'),
('file-no-change-writes',F,'if !self.is_changed() {','if false {'),
('file-is-changed',F,'self.board != *self.package.board()','false'),
('file-still-current',F,'Ok(package_guard_of(on_disk, &self.limits)? == self.guard())','Ok(package_guard_of(on_disk, &self.limits)? != self.guard())'),
('file-staged-unreferenced',F,'if refs.contains(&hash) && !there.contains(&hash.blob_name()) {','if !there.contains(&hash.blob_name()) {'),
('file-staged-listed',F,'if refs.contains(&hash) && !there.contains(&hash.blob_name()) {','if refs.contains(&hash) {'),
('file-held-keeps-ops',F,'group: group.clone(),','group: Group { ops: Vec::new(), ..group.clone() },'),
('file-revs-all',F,'if before.get(&o.id).copied() != Some(o) {','if true {'),
('file-revs-wrong',F,'revs.insert(o.id.clone(), o.rev()?);','revs.insert(o.id.clone(), Sha256::of(b""));'),
('file-read-staged',F,'if let Some((media_type, bytes)) = self.staged.get(hash) {','if let Some((media_type, bytes)) = None::<&(String, Vec<u8>)> {'),
# the target and the echo
('target-empty-accepted',F,'if path.is_empty() {','if false {'),
('echo-kind',F,'kind: crate::FORMAT,','kind: "board",'),
('echo-path',F,'path: target.0.clone(),','path: target.0.trim().to_string(),'),
('echo-version-read',F,'On::file(&self.target, Some(self.guard()))','On::file(&self.target, None)'),
('echo-version-written',F,'Ok(Saved::Wrote { written, .. }) => written.package_guard,','Ok(Saved::Wrote { .. }) => read,'),
('echo-version-open',F,'on: On::file(&target, None),','on: On::file(&target, Some(Sha256::of(b""))),'),
('echo-dropped',F,'on: self.on(),','on: On { kind: "", path: String::new(), version: None, mode: Mode::File },'),
('echo-dropped-open',F,'on: On::file(&target, None),','on: On { kind: "", path: String::new(), version: None, mode: Mode::File },'),
('echo-dropped-save',F,'let on = On::file(&target, Some(version));','let on = { let _ = version; On { kind: "", path: String::new(), version: None, mode: Mode::File } };'),
('fields-unknown-type','crates/board/src/verbs/read.rs','super::VerbCode::UnknownType,','super::VerbCode::BadValue,'),
('connect-known-fields','crates/board/src/verbs/edit.rs','known_fields(&ObjectType::Connector, fields.keys().map(String::as_str))?;',''),
# read
('read-filter',R,'kind.map_or(true, |k| *k == o.kind)','{ let _ = kind; true }'),
('read-list-position',R,'out.push(view(o, i)?);','out.push(view(o, 0)?);',0),
('read-search-position',R,'out.push(view(o, i)?);','out.push(view(o, 0)?);',1),
('read-refs-to',R,'.filter(|r| r.from == *id || r.target == *id)','.filter(|r| r.from == *id)'),
('read-refs-from',R,'.filter(|r| r.from == *id || r.target == *id)','.filter(|r| r.target == *id)'),
('read-case-haystack',R,'out.to_lowercase()','out'),
('read-case-needle',R,'text.trim().to_lowercase()','text.trim().to_string()'),
('read-trim',R,'text.trim().to_lowercase()','text.to_lowercase()'),
('read-empty-search',R,'if wanted.is_empty() {','if false {'),
('read-search-text',R,'["text", "title", "label", "assignee", "status"]','["title", "label", "assignee", "status"]'),
('read-search-title',R,'["text", "title", "label", "assignee", "status"]','["text", "label", "assignee", "status"]'),
('read-search-label',R,'["text", "title", "label", "assignee", "status"]','["text", "title", "assignee", "status"]'),
('read-search-assignee',R,'["text", "title", "label", "assignee", "status"]','["text", "title", "label", "status"]'),
('read-search-status',R,'["text", "title", "label", "assignee", "status"]','["text", "title", "label", "assignee"]'),
('read-search-tags',R,'tags.iter().for_each(|t| push_text(&mut out, Some(t)));','let _ = tags;'),
('read-search-cells',R,'cells.values().for_each(|c| push_text(&mut out, Some(c)));','let _ = cells;'),
('read-search-replies',R,'push_text(&mut out, r.get("text"));','let _ = r;'),
('read-fields-one',R,'if let Some(name) = type_name {','if let Some(name) = None::<&str> {'),
('read-state-json',R,'RefState::Orphaned => "orphaned",','RefState::Orphaned => "attached",'),
('read-hint-json',R,'f.insert("hint".into(), Value::Obj(h));','let _ = h;'),
('read-rev-json',R,'f.insert("rev".into(), Value::Str(self.rev.to_string()));','',0),
# delete and its rules
('del-names-locked',E,'self.removed_locked.push(o.id.clone());',''),
('del-names-all',E,'if is_locked(o) {','if true {'),
('del-unlock-first',E,'self.set_op(&o.id, rev, one("locked", Value::Bool(false)), Vec::new());',''),
('del-no-cascade',E,'let deps = if cascade {','let deps = if false {'),
('del-always-cascade',E,'let deps = if cascade {','let deps = if true {'),
('dep-from',E,'targets.extend(end_id(o.fields.get("from")));',''),
('dep-to',E,'targets.extend(end_id(o.fields.get("to")));',''),
('dep-mind',E,'ObjectType::Mind => targets.extend(str_of(o, "parent")),','ObjectType::Mind => {}'),
('dep-comment',E,'ObjectType::Comment => targets.extend(str_of(o, "target")),','ObjectType::Comment => {}'),
('dep-walk',E,'walk.push((*dep).clone());',''),
('del-removed-order',E,'if o.id == *id || deps.contains(&o.id) {','if deps.contains(&o.id) {'),
('del-orphans-none',E,'if target_goes && !holder_goes {','if false {'),
('del-orphans-holder',E,'if target_goes && !holder_goes {','if target_goes {'),
('del-orphan-state',E,'state: RefState::Orphaned,','state: RefState::Attached,'),
('del-gone-early',E,'if !self.index.contains_key(id.as_str()) {','if false {'),
('put-back-relock-flag',E,'let relock = is_frame && is_locked(&object);','let relock = false;'),
('put-back-relock-op',E,'self.set_op(&id, rev, one("locked", Value::Bool(true)), Vec::new());',''),
('put-back-frame-order',E,'if is_frame {','if false {'),
('put-back-frame-after',E,'Some(at) => frames[..at]\n                        .iter()\n                        .rev()\n                        .take(NEIGHBOURS)\n                        .cloned()\n                        .collect(),','Some(_) => Vec::new(),'),
('put-back-z-after',E,'at: Position::After(removed.after.clone()),','at: Position::Front,'),
# connect, react, vote, comment
('connect-end-there',E,'End::Object { id, anchor } => {\n                self.obj(id)?;','End::Object { id, anchor } => {'),
('connect-anchor-auto',E,'unwrap_or_else(|| "auto".to_string())','unwrap_or_default()'),
('connect-own-ends',E,'for name in ["from", "to"] {\n            if fields.contains_key(name) {','for name in ["from", "to"] {\n            if false && fields.contains_key(name) {'),
('react-on',E,'if on {','if !on {'),
('react-keeps-mine',E,'if has {','if false {'),
('react-replaces-others',E,'if a.iter().any(|v| matches!(v, Value::Str(t) if t == token.as_str())));','if !a.is_empty());'),
('vote-zero',E,'if delta == 0 {','if false {'),
('vote-active',E,'if voting.get("active") != Some(&Value::Bool(true)) {','if false {'),
('vote-off-absent',E,'let Some(Value::Obj(voting)) = self.board.settings.get("voting") else {\n            return Err(refuse(VerbCode::VotingOff, "voting isn\'t on"));\n        };','let empty = Fields::new();\n        let voting = match self.board.settings.get("voting") {\n            Some(Value::Obj(v)) => v,\n            _ => &empty,\n        };\n        if voting.is_empty() {\n            return Ok(());\n        }'),
('vote-cap-both-ways',E,'if delta > 0 {','if true {'),
('vote-cap-off',E,'if used + f64::from(delta) > *per {','if false {'),
('vote-cap-edge',E,'if used + f64::from(delta) > *per {','if used + f64::from(delta) >= *per {'),
('vote-cap-this-object',E,'let used: f64 = self\n                    .board\n                    .objects\n                    .iter()','let used: f64 = self\n                    .board\n                    .objects\n                    .iter()\n                    .filter(|x| x.id != *id)'),
('vote-adds',E,'let next = Self::votes_of(o, token.as_str()) + f64::from(delta);','let next = f64::from(delta);'),
('vote-zero-entry',E,'let entry = if next > 0.0 {','let entry = if true {'),
('comment-text',E,'if text.is_empty() {','if false {'),
('comment-place',E,'(at("x") + w - 12.0, at("y") - 20.0)','(at("x"), at("y"))'),
('comment-target-kept',E,'fields.insert("target".into(), Value::Str(t.to_string()));',''),
('comment-target-there',E,'if let Some(t) = target {\n            self.obj(t)?;','if let Some(t) = target {'),
('reply-type',E,'if o.kind != ObjectType::Comment {','if false {',0),
('reply-time',E,'let Some(at) = self.cx.at.clone() else {\n            return Err(refuse(VerbCode::NeedsActor, "a reply needs the time"));\n        };','let at = self.cx.at.clone().unwrap_or_else(|| "2026-01-01T00:00:00Z".to_string());'),
('reply-keeps',E,'let mut list = list_of(o, "replies");','let mut list = Vec::new();'),
('reply-by',E,'item.insert("by".into(), Value::Str(token.to_string()));','item.insert("by".into(), Value::Str(token.to_string()));\n        item.insert("by".into(), s("p-2"));'),
('resolve-type',E,'if o.kind != ObjectType::Comment {','if false {',1),
('lock-skip-same',E,'if is_locked(o) == locked {','if false {'),
# items
('check-type',E,'if schema::field_of(o.kind.as_str(), "checks").is_none() {','if false {'),
('check-set-text',E,'f.insert("text".into(), s(text));','let _ = text;'),
('check-set-done',E,'f.insert("done".into(), Value::Bool(done));','let _ = done;'),
('check-new-done',E,'item.insert("done".into(), Value::Bool(false));','item.insert("done".into(), Value::Bool(true));'),
('table-type',E,'if o.kind != ObjectType::Table {','if false {'),
('table-column-there',E,'place_of(id, &list_of(o, "columns"), column, "column")?;',''),
('table-row-after',E,'Some(id) => Ok(place_of(object, list, id, what)? + 1),','Some(id) => Ok(place_of(object, list, id, what)?),'),
('table-column-cells',E,'cells.remove(column);','let _ = cells;'),
('table-new-three',E,'for _ in 0..3 {\n                columns.push','for _ in 0..2 {\n                columns.push'),
('table-own',E,'&& !fields.contains_key("columns")\n','')
,
('add-id-field',G,'for name in ["id", "type"] {\n        if fields.contains_key(name) {','for name in ["id", "type"] {\n        if false && fields.contains_key(name) {'),
# the planner
('me-kind',G,'p.name.as_deref() == Some(name) && p.kind.as_deref() == kind','p.name.as_deref() == Some(name)'),
('me-name',G,'p.name.as_deref() == Some(name) && p.kind.as_deref() == kind','p.kind.as_deref() == kind'),
('me-mints-name',G,'name: name.map(str::to_string),','name: None,'),
('plan-adds-person',G,'ops.push(Op::AddPerson {\n                token: token.clone(),\n                person: person.clone(),\n            });','let _ = (token, person);'),
('plan-by',G,'by: self.cx.by.as_ref().map(|m| m.token.clone()),','by: None,'),
('mint-on-board',G,'if !self.index.contains_key(id.as_str()) && self.taken.insert(id.clone()) {','if self.taken.insert(id.clone()) {'),
('mint-in-plan',G,'if !self.index.contains_key(id.as_str()) && self.taken.insert(id.clone()) {','if !self.index.contains_key(id.as_str()) {'),
('mint-length',G,'hex(&random[..5])','hex(&random[..4])'),
('stamp-keeps',G,'fields.entry("created".to_string()).or_insert(Value::Obj(c));','fields.insert("created".to_string(), Value::Obj(c));'),
('stamp-off',G,'fields.entry("created".to_string()).or_insert(Value::Obj(c));','let _ = c;'),
('needs-actor',G,'None => Err(refuse(\n                VerbCode::NeedsActor,\n                "this needs to know who\'s acting",\n            )),','None => Ok(Id::new("p-1")?),'),
# arrange
('box-default',A,'match schema::field_of(o.kind.as_str(), name).and_then(|f| f.default) {','match None::<FieldDefault> {'),
('box-pen',A,'x: x + x1,','x,'),
('box-connector',A,'if o.kind == ObjectType::Connector {\n        return None;\n    }',''),
('drawn-rotate',A,'if angle == 0.0 {','if true {'),
('move-rounds',A,'set.insert("x".into(), Value::Num(num_field(o, "x") + x_by));','set.insert("x".into(), Value::Num(js_round(num_field(o, "x") + x_by)));'),
('place-same-x',A,'if x_by != 0.0 {','if true {'),
('move-group',A,'if in_group && o.kind != ObjectType::Connector && !named.contains(&o.id) {','if false {'),
('move-frame-field',A,'Some(Value::Str(f)) => f == frame.id.as_str(),','Some(Value::Str(_)) => false,'),
('move-frame-other-field',A,'Some(Value::Str(f)) => f == frame.id.as_str(),','Some(Value::Str(_)) => true,'),
('move-frame-inside',A,'_ => box_of(o).is_some_and(|b| inside(b, fb)),','_ => false,'),
('move-frame-partly',A,'a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h','intersects(a, b)'),
('move-frame-carries-frame',A,'|| o.kind == ObjectType::Frame\n','')
,
('move-frame-carries-line',A,'|| o.kind == ObjectType::Connector\n','')
,
('move-locked-stays',A,'if is_locked(o) {','if false {'),
('order-front-keeps',A,'to: at,\n                });\n                at = Position::After(vec![o.id.clone()]);','to: at.clone(),\n                });'),
('order-after',A,'Layer::After(names) => chain(Position::After(names.clone()), &mut self.ops),','Layer::After(_) => chain(Position::Front, &mut self.ops),'),
('order-back',A,'Layer::Back => chain(Position::Back, &mut self.ops),','Layer::Back => chain(Position::Front, &mut self.ops),'),
('order-overlap',A,'&& box_of(o).is_some_and(|b| intersects(b, selected))',''),
('order-frame-counts',A,'o.kind != ObjectType::Frame\n                        && !named.contains(&o.id)','!named.contains(&o.id)'),
('order-forward-start',A,'objects[highest..].iter().find(|o| overlaps(o))','objects.iter().find(|o| overlaps(o))'),
('order-backward-below',A,'let first = match i.checked_sub(1) {\n                        Some(below) => Position::After(vec![objects[below].id.clone()]),\n                        None => Position::Back,\n                    };','let first = Position::After(vec![objects[i].id.clone()]);'),
('order-backward-bottom',A,'None => Position::Back,','None => Position::Front,'),
('group-few',A,'if ids.len() < 2 {\n            return Err(refuse_with(\n                VerbCode::TooFew,\n                "a group needs','if false {\n            return Err(refuse_with(\n                VerbCode::TooFew,\n                "a group needs'),
('ungroup-skip',A,'if self.obj(&t.id)?.fields.contains_key("group") {','if true {'),
('align-few',A,'if ids.len() < 2 {\n            return Err(refuse_with(\n                VerbCode::TooFew,\n                "lining up','if false {\n            return Err(refuse_with(\n                VerbCode::TooFew,\n                "lining up'),
('align-left',A,'AlignTo::Left => (all.x - b.x, 0.0),','AlignTo::Left => (0.0, 0.0),'),
('align-right',A,'AlignTo::Right => (js_round(all.x + all.w - b.w) - b.x, 0.0),','AlignTo::Right => (js_round(all.x + all.w) - b.x, 0.0),'),
('align-center',A,'AlignTo::Center => (js_round(all.x + all.w / 2.0 - b.w / 2.0) - b.x, 0.0),','AlignTo::Center => (js_round(all.x + all.w / 2.0) - b.x, 0.0),'),
('align-top',A,'AlignTo::Top => (0.0, all.y - b.y),','AlignTo::Top => (0.0, 0.0),'),
('align-bottom',A,'AlignTo::Bottom => (0.0, js_round(all.y + all.h - b.h) - b.y),','AlignTo::Bottom => (0.0, js_round(all.y + all.h) - b.y),'),
('align-middle',A,'AlignTo::Middle => (0.0, js_round(all.y + all.h / 2.0 - b.h / 2.0) - b.y),','AlignTo::Middle => (0.0, js_round(all.y + all.h / 2.0) - b.y),'),
('dist-few',A,'if ids.len() < 3 {','if false {'),
('dist-sort',A,'list.sort_by(|a, b| start(&a.2).total_cmp(&start(&b.2)));',''),
('dist-gap',A,'pos += size(b) + gap;','pos += gap;'),
('dist-axis',A,'let across = axis == Axis::Horizontal;','let across = true;'),
('tidy-stays',A,'Some(b) if !TIDY_STAYS.contains(&&o.kind) => {','Some(b) => {'),
('tidy-few',A,'if items.len() < 2 {','if false {'),
('tidy-row-order',A,'row.sort_by(|a, b| a.b.x.total_cmp(&b.b.x));',''),
('tidy-rows',A,'Some((cy, row)) if centre(&item) - *cy < mid * 0.6 => row.push(item),','Some((_, row)) => row.push(item),'),
('tidy-gap',A,'x += i.b.w + GAP;','x += i.b.w;'),
('tidy-row-gap',A,'y += row_h + GAP;','y += row_h;'),
('style-kind',A,'if a.kind == b.kind && o.fields.get(name) != Some(v) {','if o.fields.get(name) != Some(v) {'),
('style-same',A,'if a.kind == b.kind && o.fields.get(name) != Some(v) {','if a.kind == b.kind {'),
('mind-type',A,'if p.kind != ObjectType::Mind {','if false {'),
('mind-left-of-root',A,'px < num_field(root, "x")','px > num_field(root, "x")'),
('mind-lighter',A,'on_left < kids.len() - on_left','on_left > kids.len() - on_left'),
('mind-below-kids',A,'}) + 20.0','}) + 0.0'),
('mind-beside',A,'num_field(p, "y") + num_field(p, "h") / 2.0 - 24.0','num_field(p, "y")'),
('mind-colour-parent',A,'p.fields.get("color").cloned()','None'),
('mind-colour-count',A,'Some(s(COLOURS[kids.len() % COLOURS.len()]))','Some(s(COLOURS[0]))'),
('mind-loop',A,'if hops > self.board.objects.len() {','if false {'),
# copies
('clone-no-votes',N,'for gone in ["votes", "reactions", "created"] {','for gone in ["reactions", "created"] {'),
('clone-no-reactions',N,'for gone in ["votes", "reactions", "created"] {','for gone in ["votes", "created"] {'),
('clone-no-created',N,'for gone in ["votes", "reactions", "created"] {','for gone in ["votes", "reactions"] {'),
('clone-connector-stays',N,'if o.kind != ObjectType::Connector {\n                fields.insert("x".into(), Value::Num(js_round(num_field(o, "x") + dx)));','if true {\n                fields.insert("x".into(), Value::Num(js_round(num_field(o, "x") + dx)));'),
('clone-group-new',N,'fields.insert("group".into(), Value::Str(new));','let _ = new;'),
('clone-ends',N,'fields.insert(name.to_string(), self.cloned_end(end, &map, dx, dy));','let _ = (end, name);'),
('clone-parent-outside',N,'None => {\n                        fields.remove(name);\n                    }','None => {}'),
('clone-frame',N,'fields.insert("frame".into(), Value::Str(new));','let _ = new;'),
('end-hint-gone',N,'out.remove("hint");',''),
('end-outside-offset',N,'js_round(b.x + b.w / 2.0 + dx),','js_round(b.x + b.w / 2.0),'),
('end-free-offset',N,'out.insert("x".into(), Value::Num(at("x") + dx));','out.insert("x".into(), Value::Num(at("x")));'),
('dup-inside-both',N,'["from", "to"].iter().all(|name| {','["from", "to"].iter().any(|name| {'),
('dup-order',N,'.filter(|o| named.contains(&o.id) || both_inside(o))','.filter(|o| named.contains(&o.id))'),
('paste-empty',N,'let b = bounds(held)\n            .ok_or_else(|| refuse_with(VerbCode::TooFew, "nothing to paste", Facts::min(1)))?;','let b = bounds(held).unwrap_or(super::arrange::Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });'),
('paste-centre',N,'let dx = js_round(x - b.x - b.w / 2.0);','let dx = js_round(x - b.x);'),
('quick-type',N,'if !QUICK.contains(&&o.kind) {','if false {'),
('quick-locked',N,'if is_locked(o) {','if false {'),
('quick-keeps-text',N,'fields.insert((*name).to_string(), s(""));','let _ = name;'),
('quick-unknown',N,'if known && !dropped {','if !dropped {'),
('quick-dropped',N,'if known && !dropped {','if known {'),
('quick-frame-blocks',N,'ObjectType::Frame,\n                        ObjectType::Pen,','ObjectType::Pen,'),
('quick-steps',N,'if free(x, y) {','if true {'),
('quick-side',N,'Side::Left => (-1.0, 0.0, Side::Right),','Side::Left => (1.0, 0.0, Side::Right),'),
('quick-opposite',N,'Side::Right => (1.0, 0.0, Side::Left),','Side::Right => (1.0, 0.0, Side::Right),'),
# templates
('tpl-rounds',T,'Value::Num(((v * 10.0) + 0.5).floor() / 10.0)','Value::Num(v)'),
('tpl-name',T,'_ => Err(refuse_with(\n                VerbCode::NoTemplate,\n                format!("no template {name:?}"),\n                Facts {\n                    name: Some(name.to_string()),\n                    ..Facts::default()\n                },\n            )),','_ => Ok(()),'),
('tpl-flow-link',T,'self.link(&c, &d, "Yes", ("bottom", "top"), "elbow")?;','self.link(&c, &d, "Yes", ("auto", "auto"), "elbow")?;'),
# raw
('raw-only',W,'if !names.contains(&key.as_str()) {','if false {'),
('raw-z-default',W,'None => Ok(Position::Front),','None => Ok(Position::Back),'),
('raw-back',W,'Some(Value::Str(s)) if s == "back" => Ok(Position::Back),','Some(Value::Str(s)) if s == "back" => Ok(Position::Front),'),
('raw-after',W,'Ok(Position::After(ids))','Ok(Position::Front)'),
('raw-add-removes-id',W,'fields.remove("id");',''),
('raw-add-removes-type',W,'fields.remove("type");',''),
('raw-person-extra',W,'extra: p,','extra: Fields::new(),'),
('raw-label-twice',W,'if labels.insert(label.clone(), fresh.clone()).is_some() {','if labels.insert(label.clone(), fresh.clone()).is_some() && false {'),
('raw-label-names',W,'for name in ["parent", "target", "frame"] {','for name in ["target", "frame"] {'),
('raw-label-target',W,'for name in ["parent", "target", "frame"] {','for name in ["parent", "frame"] {'),
('raw-label-frame',W,'for name in ["parent", "target", "frame"] {','for name in ["parent", "target"] {'),
('raw-label-ends',W,'for name in ["from", "to"] {\n        if let Some(Value::Obj(end)) = f.get_mut(name) {','for name in ["to"] {\n        if let Some(Value::Obj(end)) = f.get_mut(name) {'),
('raw-label-op-id',W,'if let Some(v) = f.get_mut("id") {\n            relabel(v, &labels);\n        }','\n\n'),
('raw-label-f',W,'for name in ["obj", "f"] {','for name in ["obj"] {'),
('raw-label-obj',W,'for name in ["obj", "f"] {','for name in ["f"] {'),
('raw-label-at',W,'relabel_position(f.get_mut("at"), &labels);',''),
('raw-label-to',W,'relabel_position(f.get_mut("to"), &labels);',''),
('raw-label-stamp',W,'p.stamp(&mut object.fields);','let _ = object;'),
('text-num',W,'v @ Value::Num(_) => Ok(v),','v => Ok(v),'),
('text-bool',W,'_ => Err(bad(field, "must be true or false")),','_ => Ok(Value::Bool(false)),'),
('text-settings',W,'.or_else(|| schema::SETTINGS.iter().find(|f| f.name == field))',''),
('text-unknown-json',W,'return if text.starts_with([\'{\', \'[\']) {','return if false {'),
# the parity list and the verbs' doc
('parity-row-off','crates/board/src/parity.rs','Tidy, "tidy", "Tidy up", Way::Verbs(&["tidy"]);','Tidy, "tidy", "Tidy up", Way::ViewOnly("x");'),
('parity-row-wrong-verb','crates/board/src/parity.rs','Tidy, "tidy", "Tidy up", Way::Verbs(&["tidy"]);','Tidy, "tidy", "Tidy up", Way::Verbs(&["align"]);'),
('parity-row-way-kind','crates/board/src/parity.rs','Save, "save", "Save", Way::Host(','Save, "save", "Save", Way::NotInFile('),
('parity-no-why','crates/board/src/parity.rs','"It changes nothing in the file."','""',0),
('parity-doc','docs/PARITY.md','| Tidy up |','| Tidied |'),
('verbs-doc-verb','docs/VERBS.md','`inverse`','`inverted`'),
('verbs-doc-code','docs/VERBS.md','`no_votes_left`','`no votes left`'),
('verbs-doc-template','docs/VERBS.md','`brainstorm`','`storm`'),
('verb-list-short',G,'    "tidy",\n',''),
('verb-name',G,'Verb::Tidy { .. } => "tidy",','Verb::Tidy { .. } => "tidied",'),
# what the setup-failing tests asked for: a catch at their own asserts
('comment-point',E,'(_, Some(p)) => p,','(_, Some(_)) => (0.0, 0.0),'),
('connect-point',E,'let mut f = one("x", Value::Num(*x));','let mut f = one("x", Value::Num(*x + 1.0));'),
('file-read-media',F,'media_type: media_type.clone(),','media_type: String::new(),'),
('file-keeps-parts','crates/board/src/package.rs','.filter(|p| p.role != ROLE_BOARD)','.filter(|p| p.role == ROLE_BLOB)'),
('merge-extra-kept',MG,'extra.insert(name.clone(), v);','let _ = v;'),
('merge-extra-gone',MG,'Some(None) => {}','Some(None) => {\n                extra.insert(name.clone(), Value::Null);\n            }'),
# inverse
('inv-relock-add',I,'if let Some(lock) = object.fields.remove("locked") {\n                relock.insert(o.id.clone(), Some(lock));\n            }','let _ = &mut object;'),
('inv-relock-apply',I,'for (id, lock) in relock {','for (id, lock) in BTreeMap::<Id, Option<Value>>::new() {'),
('inv-relock-existing',I,'relock.insert(o.id.clone(), old.fields.get("locked").cloned());\n        } else if','relock.len();\n        } else if'),
('inv-relock-other',I,'} else if old.fields.get("locked") != o.fields.get("locked") {','} else if false {'),
('inv-unlock',I,'if locked(o) {\n            unlock.push(Op::Set {\n                id: o.id.clone(),\n                rev,\n                set: one_lock(false),\n                unset: Vec::new(),\n            });\n            relock','if false {\n            unlock.push(Op::Set {\n                id: o.id.clone(),\n                rev,\n                set: one_lock(false),\n                unset: Vec::new(),\n            });\n            relock'),
('inv-unlock-added',I,'if locked(o) {\n                unlock.push','if false {\n                unlock.push'),
('inv-clears-first',I,'if !clear.is_empty() {','if false {'),
('inv-sets-back',I,'if !set.is_empty() {','if false {'),
('inv-deletes-added',I,'dels.push(Op::Del {','let _ = (Op::Del {'),
('inv-moves',I,'ops.extend(moves);',''),
('inv-settings',I,'if before.settings != after.settings {','if false {'),
('inv-settings-unset',I,'unset.push(name.clone());',''),
]
# a host's names and forms (the pack-verbs slices): each holds one at a test's assert
VERB+=[
('raw-alias-update',W,'"set" | "update" => {','"set" => {'),
('raw-alias-delete',W,'"del" | "delete" => {','"del" => {'),
('raw-bare-list',W,'Value::Arr(ops) => Ok(one("ops", Value::Arr(ops))),','Value::Arr(_) => Ok(Fields::new()),'),
('code-no-votes-left',G,'VerbCode::NoVotesLeft => "no_votes_left",','VerbCode::NoVotesLeft => "votes_used_up",'),
('code-no-template',G,'VerbCode::NoTemplate => "no_template",','VerbCode::NoTemplate => "unknown_template",'),
('templates-all',R,'    super::TEMPLATES\n','    &super::TEMPLATES[1..]\n'),
('react-toggle',E,'let on = on.unwrap_or(!entries.contains_key(emoji));','let on = on.unwrap_or(true);'),
('react-toggle-mine',E,'let on = on.unwrap_or(!entries.contains_key(emoji));','let on = on.unwrap_or(!matches!(o.fields.get("reactions"), Some(Value::Obj(all)) if all.contains_key(emoji)));'),
('del-gone-refuses',E,'return Err(Conflict::Deleted { id: id.clone() }.into());','return Ok(());'),
('known-type',G,'if matches!(kind, ObjectType::Unknown(_)) {','if false {'),
('known-field',G,'if schema::field_of(kind.as_str(), name).is_none() {','if false {'),
('add-known-fields',E,'known_fields(kind, fields.keys().map(String::as_str))?;',''),
('update-known-fields',E,'known_fields(&o.kind, names.chain(unset.iter().map(String::as_str)))?;','let _ = names;'),
('update-known-unset',E,'names.chain(unset.iter().map(String::as_str))','names'),
('add-refuses-id',G,'for name in ["id", "type"] {','for name in ["type"] {'),
('flags-known',W,'super::known_fields(&kind, pairs.iter().map(|(name, _)| *name))?;',''),
('flags-twice',W,'if !seen.insert(*name) {','if false {',0),
('flags-null',W,'if *text == "null" {','if false {',0),
('read-search-checks',R,'push_text(&mut out, c.get("text"));','let _ = c;'),
('meta-read-settings',R,'settings: board.settings.clone(),','settings: Fields::new(),'),
('meta-read-rev',R,'rev: board.settings_rev()?,','rev: crate::hash::Sha256::of(b""),'),
('meta-json-rev',R,'f.insert("rev".into(), Value::Str(self.rev.to_string()));','',1),
('meta-json-settings',R,'f.insert("settings".into(), Value::Obj(self.settings.clone()));',''),
('code-unknown-field',G,'VerbCode::UnknownField => "unknown_field",','VerbCode::UnknownField => "field_unknown",'),
('code-unknown-type',G,'VerbCode::UnknownType => "unknown_type",','VerbCode::UnknownType => "type_unknown",'),
('host-args-react',G,'("react", &["id", "emoji", "on?"]),','("react", &["id", "emoji", "on"]),'),
('host-args-variant',G,'("delete", &["id", "rev", "cascade"]),','("delete", &["id", "rev"]),'),
]
# the host's list, as data: each takes away one thing tests/host_cli.rs holds
HCLI='crates/board/tests/data/host-cli-v1.json'
VERB+=[
('host-data-why',HCLI,'"why": "An alias of add with the type frame."','"why": ""'),
('host-data-verb',HCLI,'"pack": {"verb": "read", "verdict": "same"}','"pack": {"verb": "get", "verdict": "same"}'),
('host-data-rev',HCLI,'"flags": {"rev": "rev"},','"flags": {},'),
('host-data-host-sets',HCLI,'"host_sets": {"delta": "1: one vote each time."}','"host_sets": {}'),
('host-data-as',HCLI,'"as": {"id": "target"}','"as": {}'),
('host-data-code',HCLI,'{"code": "no_template", "pack": "no_template", "verdict": "same"},','{"code": "no_template", "pack": "unknown_template", "verdict": "same"},'),
('host-data-pack-only',HCLI,'    {"verb": "tidy", "why": "A person action with no host verb yet."},\n',''),
('host-data-form',HCLI,'"test": "tests/verbs.rs::react_with_no_on_toggles"','"test": "tests/verbs.rs::react_toggles"'),
('host-data-field-verdict',HCLI,'"timer": {"verdict": "not_file"','"title": {"verdict": "not_file"'),
]

MERGE=[
('merge-same-both',MG,'if ours == theirs {\n        return Some(ours.cloned());\n    }','if false {\n        return Some(ours.cloned());\n    }'),
('merge-take-theirs',MG,'if ours == base {\n        return Some(theirs.cloned());\n    }','if false {\n        return Some(theirs.cloned());\n    }'),
('merge-take-ours',MG,'if theirs == base {\n        return Some(ours.cloned());\n    }','if false {\n        return Some(ours.cloned());\n    }'),
('merge-last-writer',MG,'    None\n}\n\n/// What merging the objects gave','    Some(ours.cloned())\n}\n\n/// What merging the objects gave'),
('merge-kind-both-added',MG,'(None, _, _) => ConflictKind::BothAdded,','(None, _, _) => ConflictKind::BothChanged,'),
('merge-kind-ours-deleted',MG,'(_, None, _) => ConflictKind::OursDeleted,','(_, None, _) => ConflictKind::TheirsDeleted,'),
('merge-kind-theirs-deleted',MG,'(_, _, None) => ConflictKind::TheirsDeleted,','(_, _, None) => ConflictKind::OursDeleted,'),
('merge-keeps-ours',MG,'ours: oo.cloned(),','ours: None,'),
('merge-keeps-theirs',MG,'theirs: to.cloned(),','theirs: None,'),
('merge-keeps-base',MG,'base: bo.cloned(),','base: None,'),
('merge-from-ours-report',MG,'if oo == Some(&object) && oo != bo {','if false {'),
('merge-from-theirs-report',MG,'if to == Some(&object) && to != bo && oo != to {','if false {'),
('merge-from-theirs-twice',MG,'if to == Some(&object) && to != bo && oo != to {','if to == Some(&object) && to != bo {'),
('merge-deleted-report',MG,'if bo.is_some() {','if false {'),
('merge-settings-conflict',MG,'conflicts.totals.settings += 1;\n            if conflicts.room() {\n                conflicts.settings = Some(SettingsConflict {\n                    base: base.settings.clone(),\n                    ours: ours.settings.clone(),\n                    theirs: theirs.settings.clone(),\n                });\n            }','let _ = ();'),
('merge-settings-base',MG,'base: base.settings.clone(),','base: Fields::new(),'),
('merge-settings-ours',MG,'ours: ours.settings.clone(),','ours: Fields::new(),'),
('merge-settings-theirs',MG,'theirs: theirs.settings.clone(),','theirs: Fields::new(),'),
('merge-person-union',MG,'(Some(x), None) | (None, Some(x)) => Some(x.clone()),','(Some(_), None) | (None, Some(_)) => None,'),
('merge-person-from-base',MG,'(None, None) => bp.cloned(),','(None, None) => None,'),
('merge-person-conflict',MG,'conflicts.totals.people += 1;\n                    if conflicts.room() {\n                        conflicts.people.push(PersonConflict {\n                            token: token.clone(),\n                            base: bp.cloned(),\n                            ours: x.clone(),\n                            theirs: y.clone(),\n                        });\n                    }','let _ = (x, y);'),
('merge-person-base',MG,'base: bp.cloned(),','base: None,'),
('merge-person-ours',MG,'ours: x.clone(),','ours: y.clone(),'),
('merge-person-theirs',MG,'theirs: y.clone(),','theirs: x.clone(),'),
('merge-first-object',MG,'theirs: to.cloned(),\n                    });','theirs: to.cloned(),\n                    });\n                    break;'),
('merge-first-person',MG,'theirs: y.clone(),\n                        });','theirs: y.clone(),\n                        });\n                        break;'),
('merge-first-extra',MG,'theirs: theirs.extra.get(name).cloned(),\n                    });','theirs: theirs.extra.get(name).cloned(),\n                    });\n                    break;'),
('merge-extra-base',MG,'base: base.extra.get(name).cloned(),','base: None,'),
('merge-extra-ours',MG,'ours: ours.extra.get(name).cloned(),','ours: None,'),
('merge-extra-theirs',MG,'theirs: theirs.extra.get(name).cloned(),','theirs: None,'),
('merge-extra-conflict',MG,'None => {\n                conflicts.totals.extra += 1;\n                if conflicts.room() {\n                    conflicts.extra.push(ExtraConflict {\n                        name: name.clone(),\n                        base: base.extra.get(name).cloned(),\n                        ours: ours.extra.get(name).cloned(),\n                        theirs: theirs.extra.get(name).cloned(),\n                    });\n                }\n            }','None => {}'),
('merge-conflict-stops',MG,'if conflicts != MergeConflicts::default() {','if false {'),
('merge-validates',MG,'board.validate(&Limits::V1)?;',''),
('merge-limit-words',MG,'one writer at a time, whole objects, never last-writer-wins, and both edits are kept','nothing merged'),
('order-stable-longest',MG,'let slot = tails.partition_point(|t| items[*t].0 < items[i].0);','let slot = tails.len();'),
('order-takes-theirs',MG,'for id in theirs {\n        if keep.contains(*id) {','for id in theirs {\n        if true {'),
('order-ours-moved',MG,'if added || moved {','if added {'),
('order-ours-added',MG,'if added || moved {','if moved {'),
('order-top',MG,'let at = if i >= top_from {','let at = if false {'),
('order-below-none',MG,'.unwrap_or(0)','.unwrap_or(result.prev[0])'),
('order-below-update',MG,'if result.contains(id) {\n            below = Some(*id);\n        }',''),
('chain-remove-link',MG,'self.prev[after] = before;\n        }\n    }','}\n    }'),
('chain-insert-link',MG,'self.prev[after] = slot;',''),
('chain-walk',MG,'at = self.next[at];','at = self.prev[at];'),
('order-note',MG,'if moved {\n                notes.push(OrderNote {','if false {\n                notes.push(OrderNote {'),
('order-over-theirs',MG,'over_theirs: in_theirs.contains(*id) && !theirs_stable.contains(*id),','over_theirs: in_theirs.contains(*id),'),
('order-frames',MG,'let frames = merge_order(\n        &base.frames.iter().collect::<Vec<_>>(),\n        &ours.frames.iter().collect::<Vec<_>>(),','let frames = merge_order(\n        &base.frames.iter().collect::<Vec<_>>(),\n        &theirs.frames.iter().collect::<Vec<_>>(),'),
('orphans-none',MG,'.filter(|r| r.state == RefState::Orphaned)\n        .filter(|r| !was_orphaned','.filter(|r| r.state == RefState::Suspect)\n        .filter(|r| !was_orphaned'),
('orphans-old-too',MG,'.filter(|r| !was_orphaned.contains(&(r.from.clone(), r.field)))\n',''),
('base-names-nothing',MG,'let Some(named) = &copy.base else {\n        return Ok(false);','let Some(named) = &copy.base else {\n        return Ok(true);'),
('base-compare',MG,'Ok(named.sha256 == Sha256::of(&base.to_canonical_json()?))','Ok(named.sha256 != Sha256::of(&base.to_canonical_json()?))'),
('file-merge-applies',F,'self.board = merged.board.clone();',''),
('file-merge-blob-hash',F,'if Sha256::of(&bytes) != hash {','if false {'),
('file-merge-stages',F,'self.staged.insert(hash, (part.media_type.clone(), bytes));','let _ = (part, bytes);'),
('file-merge-keeps-listing',F,'if there.contains(&name) || self.staged.contains_key(&hash) {','if self.staged.contains_key(&hash) {'),
# the cap on what a merge's error keeps
('merge-cap-none',MG,'if self.kept() < MERGE_CONFLICTS_MAX {','if true {'),
('merge-cap-off-by-one',MG,'if self.kept() < MERGE_CONFLICTS_MAX {','if self.kept() <= MERGE_CONFLICTS_MAX {'),
('merge-cap-flag',MG,'self.truncated = true;','let _ = ();'),
('merge-cap-total-settings',MG,'conflicts.totals.settings += 1;',''),
('merge-cap-total-extra',MG,'conflicts.totals.extra += 1;',''),
('merge-cap-total-people',MG,'conflicts.totals.people += 1;',''),
('merge-cap-total-objects',MG,'conflicts.totals.objects += 1;',''),
('merge-cap-order-settings',MG,'let settings = merge_settings(base, ours, theirs, &mut conflicts);\n    let extra = merge_extra(base, ours, theirs, &mut conflicts);','let extra = merge_extra(base, ours, theirs, &mut conflicts);\n    let settings = merge_settings(base, ours, theirs, &mut conflicts);'),
('merge-cap-order-extra',MG,'let extra = merge_extra(base, ours, theirs, &mut conflicts);\n    let people = merge_people(base, ours, theirs, &mut conflicts);','let people = merge_people(base, ours, theirs, &mut conflicts);\n    let extra = merge_extra(base, ours, theirs, &mut conflicts);'),
('merge-cap-order-people',MG,'let people = merge_people(base, ours, theirs, &mut conflicts);\n    let merged = merge_objects(base, ours, theirs, &mut conflicts);','let merged = merge_objects(base, ours, theirs, &mut conflicts);\n    let people = merge_people(base, ours, theirs, &mut conflicts);'),
('merge-cap-order-ids',MG,'for id in &ids {','for id in ids.iter().rev() {'),
('merge-cap-display-shown',MG,'self.kept(),\n            t.all()\n        )','t.all(),\n            t.all()\n        )'),
]
# the header read: a host's listing of files without an open
MUT+=[
('stream-past-declared',Z,'if written + r.bytes_written as u64 > declared {','if written > declared {'),
]
HD='crates/board/src/header.rs'
HEADER=[
('header-reads-board',HD,'let (_, manifest) = package::open_index(&mut src, l)?;','let (e, manifest) = package::open_index(&mut src, l)?;\n    let _ = e.get(NAME_BOARD).map(|b| zip::read_entry(&mut src, b, l));'),
('header-guard',HD,'Ok(Sha256::of(&manifest.to_canonical_json()?))','Ok(Sha256::of(b""))'),
('header-preview-size',HD,'.find(|p| p.role == ROLE_PREVIEW)\n            .map(|p| p.bytes),','.find(|p| p.role == ROLE_PREVIEW)\n            .map(|p| p.bytes + 1),'),
('preview-bound',HD,'if part.bytes > max_bytes {','if false {'),
('preview-bound-edge',HD,'if part.bytes > max_bytes {','if part.bytes >= max_bytes {'),
('preview-hash',HD,'if Sha256::of(&bytes) != part.sha256 {','if false {'),
('summary-hash',HD,'if hasher.finalize().as_slice() != part.sha256.as_bytes() {','if false {'),
('summary-count-nested',HD,'if parent == Some((Role::Objects, false)) {','if matches!(parent, Some((Role::Objects | Role::Other, false))) {'),
('summary-count-one',HD,'self.objects += 1;','self.objects = 1;'),
('title-place',HD,'(Role::Top, Member::Settings) => Role::Settings,','(Role::Top, Member::Settings | Member::Other) => Role::Settings,'),
('title-limit',HD,'if self.title_chars > self.title_max {','if false {'),
('title-limit-edge',HD,'if self.title_chars > self.title_max {','if self.title_chars >= self.title_max {'),
('title-kind',HD,"if title && b != b'\"' {",'if false {'),
('depth-bound',HD,'if self.stack.len() as u64 + 1 > self.l.depth {','if false {'),
('values-bound',HD,'if self.values > self.l.values {','if false {'),
('objects-bound',HD,'if self.objects > self.l.objects {','if false {'),
('scan-shape',HD,'if b != want {','if false {'),
('scan-missing',HD,'if !seen {','if false {'),
('scan-twice',HD,'if self.seen[i] {','if false {'),
('scan-after-doc',HD,'return Err(syntax(self.at, "content after the document"));','return Ok(());'),
('scan-ends-early',HD,'if self.state != St::Between || !self.done {','if self.state != St::Between {'),
('scan-utf8',HD,'_ => Err(self.not_utf8()),','_ => self.lead(b, 1, 0x80, 0xBF),'),
('scan-utf8-continuation',HD,'if !(self.utf_lo..=self.utf_hi).contains(&b) {','if false {'),
('scan-lone-low',HD,'0xDC00..=0xDFFF => return Err(self.surrogate()),','0xDC00..=0xDFFF => {}'),
('scan-control',HD,'0..=0x1F => Err(syntax(self.at, "control character in a string")),','0..=0x1F => Ok(()),'),
('scan-leading-zero',HD,'(Num::Zero, _) if digit => return Err(syntax(self.at, "bad number")),','(Num::Zero, _) if digit => Some(Num::Int),'),
('scan-escape',HD,'_ => return Err(syntax(self.at, "unknown escape")),','_ => b\'?\','),
]

# A check held by two layers: no one edit reaches the vector, so a mutant takes both away. Every edit
# has to apply or it's stale, and the named vector has to be in the failure text or it isn't a catch.
# Each of these takes something a file should keep, and the corpus has to notice with no other test.
FID=[
('fid-part-dropped','crates/board/src/package.rs','.filter(|p| p.role != ROLE_BOARD)','.filter(|p| p.role == ROLE_BLOB)'),
('fid-unknown-top',M,'let mut top = self.extra.clone();','let mut top = Fields::new();'),
('fid-number-rounded',J,'if v == 0.0 {\n        return Err(inexact("is too small for a double, and would become zero"));\n    }\n    Ok(v)','if v == 0.0 {\n        return Err(inexact("is too small for a double, and would become zero"));\n    }\n    Ok((v * 10.0).round() / 10.0)'),
('fid-edit-touches-neighbour',O,'let mut next = current.clone();\n        for (name, value) in set {','let mut next = current.clone();\n        next.fields.remove("x");\n        for (name, value) in set {'),
('fid-lineage',P,'lineage: self.lineage,','lineage: Lineage::from_bytes([0; 16]),'),
('fid-cell-limit',S,'pub const CELL_MAX_CHARS: usize = 2000;','pub const CELL_MAX_CHARS: usize = 1999;'),
('fid-points-limit',S,'pub const POINTS_MAX: usize = 5000;','pub const POINTS_MAX: usize = 4999;'),
('fid-replies-limit',S,'pub const REPLIES_MAX: usize = 1000;','pub const REPLIES_MAX: usize = 999;'),
('fid-order-lost',M,'let mut objects = Vec::with_capacity(objects_in.len());\n        for (i, v) in objects_in.into_iter().enumerate() {','let mut objects = Vec::with_capacity(objects_in.len());\n        for (i, v) in objects_in.into_iter().rev().enumerate() {'),
('fid-undo-field',I,'ops.extend(restore);',''),
('fid-lock-ignored',O,'if is_locked(current) {\n            // The same rule','if false {\n            // The same rule'),
('fid-verb-breaks-board',E,'fields.insert("x".into(), Value::Num(x));','fields.insert("x".into(), Value::Str(String::new()));'),
]
# The bridge (bridge/, its own workspace, on the kit): one section per part of it. A bridge mutant's
# catch is read with its paths under bridge/ (catch.locations).
BR='bridge/src/rows.rs'; BP='bridge/src/report.rs'; BV='bridge/src/value.rs'
BRIDGE_ROWS=[  # rows, reports and value_out (bridge/src/rows.rs, report.rs, value.rs)
# rows: a format refusal's row by phase
('br-file-phases',BR,'let file = matches!(phase, Phase::Open | Phase::GuardRead);','let file = matches!(phase, Phase::Open);'),
('br-limit-file',BR,'if file {\n                "MPC-E846"','if false {\n                "MPC-E846"'),
('br-json-file',BR,'if file {\n                "MPC-E845"\n            } else if phase == Phase::Ops {','if false {\n                "MPC-E845"\n            } else if phase == Phase::Ops {'),
('br-json-ops',BR,'} else if phase == Phase::Ops {','} else if false {'),
('br-ids-file',BR,'if file {\n                "MPC-E845"\n            } else {\n                "MPC-E847"','if false {\n                "MPC-E845"\n            } else {\n                "MPC-E847"'),
('br-zip-row',BR,'| C::ZipDeflateInvalid => "MPC-E842",','| C::ZipDeflateInvalid => "MPC-E843",'),
('br-manifest-row',BR,'| C::PartRole => "MPC-E843",','| C::PartRole => "MPC-E842",'),
('br-newer-row',BR,'C::MinReaderTooHigh => "MPC-E844",','C::MinReaderTooHigh => "MPC-E845",'),
('br-blob-row',BR,'C::BlobMissing => "MPC-E841",','C::BlobMissing => "MPC-E848",'),
('br-limit-raised',BR,'C::LimitRaised => "MPC-E848",','C::LimitRaised => "MPC-E846",'),
('br-locked-row',BR,'C::ObjectLocked => "MPC-E835",','C::ObjectLocked => "MPC-E847",'),
('br-io-row',BR,'C::IoError => "MPC-E857",','C::IoError => "MPC-E848",'),
# rows: each verb code's
('br-verb-needs-actor',BR,'VerbCode::NeedsActor => "MPC-E855",','VerbCode::NeedsActor => "MPC-E857",'),
('br-verb-voting-off',BR,'VerbCode::VotingOff => "MPC-E836",','VerbCode::VotingOff => "MPC-E837",'),
('br-verb-no-votes',BR,'VerbCode::NoVotesLeft => "MPC-E837",','VerbCode::NoVotesLeft => "MPC-E836",'),
('br-verb-wrong-type',BR,'VerbCode::WrongType => "MPC-E853",','VerbCode::WrongType => "MPC-E857",'),
('br-verb-too-few',BR,'VerbCode::TooFew => "MPC-E854",','VerbCode::TooFew => "MPC-E857",'),
('br-verb-no-template',BR,'VerbCode::NoTemplate => "MPC-E839",','VerbCode::NoTemplate => "MPC-E838",'),
('br-verb-unknown-field',BR,'VerbCode::UnknownField => "MPC-E850",','VerbCode::UnknownField => "MPC-E851",'),
('br-verb-unknown-type',BR,'VerbCode::UnknownType => "MPC-E851",','VerbCode::UnknownType => "MPC-E850",'),
('br-verb-no-item',BR,'VerbCode::NoSuchItem => "MPC-E840",','VerbCode::NoSuchItem => "MPC-E838",'),
('br-verb-no-free-id',BR,'VerbCode::NoFreeId => "MPC-E856",','VerbCode::NoFreeId => "MPC-E857",'),
('br-verb-bad-value',BR,'VerbCode::BadValue => "MPC-E852",','VerbCode::BadValue => "MPC-E847",'),
# rows: targets, conflicts, the op table
('br-target-settings',BR,'Target::Settings => TargetKind::Settings,','Target::Settings => TargetKind::Object,'),
('br-target-person',BR,'Target::Person(_) => TargetKind::Person,','Target::Person(_) => TargetKind::Object,'),
('br-changed-object',BR,'Conflict::Changed { .. } => "MPC-E830",','Conflict::Changed { .. } => "MPC-E831",'),
('br-changed-settings',BR,'} => "MPC-E858",','} => "MPC-E830",'),
('br-changed-person',BR,'} => ("changed", "person"),','} => return Ok(()),'),
('br-exists-object',BR,'Conflict::Exists { .. } => "MPC-E831",','Conflict::Exists { .. } => "MPC-E830",'),
('br-exists-person',BR,'} => "MPC-E859",','} => "MPC-E831",'),
('br-exists-settings',BR,'} => ("exists", "settings"),','} => return Ok(()),'),
('br-impossible-check',BR,'    impossible_pair(c)?;\n    row(match c {','    row(match c {'),
('br-deleted-row',BR,'Conflict::Deleted { .. } => "MPC-E838",','Conflict::Deleted { .. } => "MPC-E840",'),
('br-refused-phase',BR,'Conflict::Refused(r) => return refusal_row(r.code, phase),','Conflict::Refused(r) => return refusal_row(r.code, Phase::Input),'),
('br-op-add',BR,'Op::Add { .. } => &[(Clash::Exists, TargetKind::Object)],','Op::Add { .. } => &[(Clash::Changed, TargetKind::Object)],'),
('br-op-set-del',BR,'Op::Set { .. } | Op::Del { .. } => &[(Clash::Changed, TargetKind::Object)],','Op::Set { .. } | Op::Del { .. } => &[],'),
('br-op-meta',BR,'Op::Meta { .. } => &[(Clash::Changed, TargetKind::Settings)],','Op::Meta { .. } => &[(Clash::Exists, TargetKind::Settings)],'),
('br-op-person',BR,'Op::AddPerson { .. } => &[(Clash::Exists, TargetKind::Person)],','Op::AddPerson { .. } => &[(Clash::Changed, TargetKind::Person)],'),
('br-op-none',BR,'Op::Z { .. } | Op::FrameOrder { .. } | Op::ClearBlob { .. } => &[],','Op::Z { .. } | Op::FrameOrder { .. } | Op::ClearBlob { .. } => &[(Clash::Changed, TargetKind::Object)],'),
# report: the message, filled
('bp-fill-str',BP,'KValue::Str(s) => out.push_str(s),','KValue::Str(s) => out.push_str(&format!("{s:?}")),'),
('bp-fill-other',BP,'other => out.push_str(&other.to_json()),','other => out.push_str(&format!("{other:?}")),'),
('bp-fill-rest',BP,'    out.push_str(rest);\n    Ok(out)','    Ok(out)'),
('bp-fill-prefix',BP,'out.push_str(&rest[..open]);',''),
# report: a format refusal's keys
('bp-code',BP,'let mut d: Details = vec![("code", s(r.code.as_str()))];','let mut d: Details = vec![("code", s("x"))];'),
('bp-at',BP,'d.push(("at", s(at)));','',0),
('bp-hash',BP,'a.strip_prefix("blobs/")','a.strip_prefix("")'),
('bp-locked-id',BP,'a.strip_prefix("/objects/")','a.strip_prefix("")'),
('bp-locked-id-needed',BP,'let id = r.at.as_deref().and_then(|a| a.strip_prefix("/objects/"));','let id = r.at.as_deref().and_then(|a| a.strip_prefix("/objects/")).or(Some("x"));'),
('bp-needs',BP,'d.push(("needs", KValue::from(u64::from(needs))));','d.push(("needs", KValue::from(0u64)));'),
('bp-have',BP,'KValue::from(u64::from(k2_mpc_board::MIN_READER))','KValue::from(0u64)'),
('bp-io-reason',BP,'"MPC-E857" => d.push(("reason", s(&r.detail))),','"MPC-E857" => d.push(("reason", s(""))),'),
('bp-bad-ids',BP,'if !r.bad_ids.is_empty() {','if false {'),
('bp-bad-total',BP,'KValue::from(r.bad_ids_total)','KValue::from(0u64)'),
('bp-bad-truncated',BP,'KValue::Bool(r.bad_ids_truncated)','KValue::Bool(false)'),
# report: what a conflict is about
('bp-target-object',BP,'d.push(("target", s(id.as_str())));','d.push(("target", s("x")));'),
('bp-what-object',BP,'d.push(("what", s("object")));','d.push(("what", s("x")));',0),
('bp-id-object',BP,'d.push(("id", s(id.as_str())));','d.push(("id", s("x")));',0),
('bp-target-settings',BP,'d.push(("target", s("the settings")));','d.push(("target", s("x")));'),
('bp-what-settings',BP,'d.push(("what", s("settings")));','d.push(("what", s("x")));'),
('bp-target-person',BP,'d.push(("target", s(token.as_str())));','d.push(("target", s("x")));'),
('bp-what-person',BP,'d.push(("what", s("person")));','d.push(("what", s("x")));'),
('bp-id-person',BP,'d.push(("id", s(token.as_str())));','d.push(("id", s("x")));'),
('bp-theirs',BP,'d.push(("theirs", value_out(theirs)?));','d.push(("theirs", KValue::Null));'),
('bp-deleted-id',BP,'Conflict::Deleted { id } => d.push(("id", s(id.as_str()))),','Conflict::Deleted { id } => d.push(("id", s("x"))),'),
# report: a verb's own refusal's keys
('bp-verb-needs-actor',BP,'VerbCode::NeedsActor => d.push(("verb", s(verb))),','VerbCode::NeedsActor => d.push(("verb", s("x"))),'),
('bp-verb-wrong-type',BP,'d.push(("verb", s(verb)));','d.push(("verb", s("x")));',0),
('bp-verb-too-few',BP,'d.push(("verb", s(verb)));','d.push(("verb", s("x")));',1),
('bp-verb-bad-value',BP,'d.push(("verb", s(verb)));','d.push(("verb", s("x")));',2),
('bp-id-wrong-type',BP,'d.push(("id", s(id.as_str())));','d.push(("id", s("x")));',1),
('bp-type-wrong-type',BP,'d.push(("type", fact("type", &facts.type_name)?));','d.push(("type", s("x")));',0),
('bp-min',BP,'d.push(("min", KValue::from(need(entry, "min", facts.min)?)));','d.push(("min", KValue::from(9u64)));'),
('bp-name',BP,'VerbCode::NoTemplate => d.push(("name", fact("name", &facts.name)?)),','VerbCode::NoTemplate => d.push(("name", s("x"))),'),
('bp-type-unknown-field',BP,'d.push(("type", fact("type", &facts.type_name)?));','d.push(("type", s("x")));',1),
('bp-field',BP,'d.push(("field", fact("field", &facts.field)?));','d.push(("field", s("x")));'),
('bp-type-unknown-type',BP,'VerbCode::UnknownType => d.push(("type", fact("type", &facts.type_name)?)),','VerbCode::UnknownType => d.push(("type", s("x"))),'),
('bp-id-no-item',BP,'d.push(("id", s(id.as_str())));','d.push(("id", s("x")));',2),
('bp-item',BP,'d.push(("item", fact("item", &facts.item)?));','d.push(("item", s("x")));'),
('bp-reason-bad-value',BP,'d.push(("reason", s(detail)));','d.push(("reason", s("x")));'),
('bp-verb-conflict-phase',BP,'VerbError::Conflict(c) => conflict_report(c, phase),','VerbError::Conflict(c) => conflict_report(c, Phase::Input),'),
# report: a merge's conflicts
('bp-merge-settings',BP,'        list.push(settings_entry(c)?);\n',''),
('bp-merge-extra',BP,'        list.push(extra_entry(c)?);\n',''),
('bp-merge-people',BP,'        list.push(person_entry(c)?);\n',''),
('bp-merge-objects',BP,'        list.push(object_entry(c)?);\n',''),
('bp-merge-count',BP,'("count", KValue::from(t.all())),','("count", KValue::from(t.objects)),'),
('bp-merge-truncated',BP,'("truncated", KValue::Bool(m.truncated)),','("truncated", KValue::Bool(true)),'),
('bp-totals-settings',BP,'("settings", KValue::from(t.settings)),','("settings", KValue::from(9u64)),'),
('bp-totals-extra',BP,'("extra", KValue::from(t.extra)),','("extra", KValue::from(9u64)),'),
('bp-totals-people',BP,'("people", KValue::from(t.people)),','("people", KValue::from(9u64)),'),
('bp-totals-objects',BP,'("objects", KValue::from(t.objects)),','("objects", KValue::from(9u64)),'),
('bp-entry-id',BP,'    if let Some(id) = id {\n        o.push(("id".into(), s(id)));\n    }\n',''),
('bp-entry-kind',BP,'o.push(("kind".into(), s(kind.as_str())));','o.push(("kind".into(), s("x")));'),
('bp-entry-sides',BP,'        if let Some(v) = side {\n            o.push((name.into(), v));\n        }\n',''),
('bp-kind-both-added',BP,'(false, _, _) => ConflictKind::BothAdded,','(false, _, _) => ConflictKind::BothChanged,'),
('bp-kind-ours-deleted',BP,'(true, false, _) => ConflictKind::OursDeleted,','(true, false, _) => ConflictKind::BothChanged,'),
('bp-kind-theirs-deleted',BP,'(true, _, false) => ConflictKind::TheirsDeleted,','(true, _, false) => ConflictKind::BothChanged,'),
('bp-merge-refused-phase',BP,'MergeError::Refused(r) => refusal_report(r, Phase::Input),','MergeError::Refused(r) => refusal_report(r, Phase::Open),'),
# report: saves, held groups, the host's calls
('bp-read',BP,'("read", s(&read.to_string())),','("read", s("sha256-x")),'),
('bp-on-disk',BP,'("on_disk", s(&on_disk.to_string())),','("on_disk", s("sha256-x")),'),
('bp-save-phase',BP,'SaveError::Refused(r) => refusal_report(r, Phase::Write),','SaveError::Refused(r) => refusal_report(r, Phase::Open),'),
('bp-held-impossible',BP,'    impossible_pair(c)?;\n',''),
('bp-held-no-target',BP,'d.retain(|(k, _)| *k != "target");',''),
('bp-held-why-clash',BP,'"changed"\n            } else {\n                "exists"','"exists"\n            } else {\n                "changed"'),
('bp-held-deleted',BP,'            "deleted"','            "gone"'),
('bp-held-deleted-what',BP,'d.push(("what", s("object")));','d.push(("what", s("x")));',1),
('bp-held-deleted-id',BP,'d.push(("id", s(id.as_str())));','d.push(("id", s("x")));',3),
('bp-held-at',BP,'d.push(("at", s(at)));','',1),
('bp-held-why-code',BP,'            r.code.as_str()\n','            "x"\n'),
('bp-held-count',BP,'("count", KValue::from(held.len())),','("count", KValue::from(0usize)),'),
('bp-held-group',BP,'("group", s(&h.group.id)),','("group", s("x")),'),
('bp-no-object-id',BP,'finish(row("MPC-E838")?, vec![("id", s(id.as_str()))])','finish(row("MPC-E838")?, vec![("id", s("x"))])'),
('bp-io-host-reason',BP,'vec![("code", s("io_error")), ("reason", s(reason))],','vec![("code", s("io_error")), ("reason", s("x"))],'),
# value_out
('bv-minus-zero',BV,'let n = if n == 0.0 { 0.0 } else { n };','let n = n;'),
('bv-not-finite',BV,'KValue::float(n).ok_or_else(','KValue::float(if n.is_finite() { n } else { 0.0 }).ok_or_else('),
('bv-arr-slash',BV,"at.push('/');",'',0),
('bv-arr-index',BV,'at.push_str(&i.to_string());',''),
('bv-arr-truncate',BV,'at.truncate(len);','',0),
('bv-obj-slash',BV,"at.push('/');",'',1),
('bv-obj-name',BV,"at.push_str(&name.replace('~', \"~0\").replace('/', \"~1\"));","at.push_str(name);"),
('bv-obj-truncate',BV,'at.truncate(len);','',1),
('bv-bool',BV,'BValue::Bool(b) => KValue::Bool(*b),','BValue::Bool(b) => KValue::Bool(!*b),'),
('bv-str',BV,'BValue::Str(s) => KValue::Str(s.clone()),','BValue::Str(s) => KValue::Str(String::new()),'),
('bv-null',BV,'BValue::Null => KValue::Null,','BValue::Null => KValue::Bool(false),'),
('bv-read',BV,'    value_out(&view.to_value())\n','    let _ = view;\n    Ok(KValue::Null)\n'),
]
BE='crates/board/src/verbs/edit.rs'; BA='crates/board/src/verbs/arrange.rs'; BC='crates/board/src/verbs/clone.rs'; BT='crates/board/src/verbs/templates.rs'
BRD='crates/board/src/verbs/read.rs'; BM='crates/board/src/verbs/mod.rs'
BRIDGE_FACTS=[  # the verb refusals' facts, in the board crate, held by tests/facts.rs
('bf-wrong-type',BE,'Facts::object(&o.id, o.kind.as_str()),','Facts::default(),'),
('bf-item-object',BE,'id: Some(object.clone()),','id: None,'),
('bf-item-id',BE,'item: Some(id.to_string()),','item: None,'),
('bf-box-connector',BA,'Facts::object(&t.id, o.kind.as_str()),','Facts::default(),'),
('bf-group-min',BA,'Facts::min(2),','Facts::min(3),',0),
('bf-align-min',BA,'Facts::min(2),','Facts::min(3),',1),
('bf-distribute-min',BA,'Facts::min(3),','Facts::min(2),'),
('bf-tidy-min',BA,'Facts::min(2),','Facts::min(3),',2),
('bf-mind-parent',BA,'Facts::object(parent, p.kind.as_str()),','Facts::default(),'),
('bf-paste-min',BC,'Facts::min(1)','Facts::min(2)'),
('bf-connected-copy',BC,'Facts::object(id, o.kind.as_str()),','Facts::default(),'),
('bf-template-name',BT,'name: Some(name.to_string()),','name: None,'),
('bf-fields-type',BRD,'super::Facts::type_field(name, None),','super::Facts::default(),'),
('bf-unknown-type',BM,'Facts::type_field(kind.as_str(), None),','Facts::default(),'),
('bf-unknown-field',BM,'Facts::type_field(kind.as_str(), Some(name)),','Facts::type_field(kind.as_str(), None),'),
('bf-object-type',BM,'type_name: Some(type_name.to_string()),','type_name: None,',0),
('bf-type-field-type',BM,'type_name: Some(type_name.to_string()),','type_name: None,',1),
('bf-type-field-field',BM,'field: field.map(str::to_string),','field: None,'),
('bf-min-helper',BM,'min: Some(min),','min: None,'),
]
BW='crates/board/src/verbs/raw.rs'; BO='crates/board/src/ops.rs'
BRIDGE_ARGS=[  # what a command line needs from the verbs as data, held by tests/args.rs and raw.rs's unit test
('ba-json-args',BM,'    (\n        "inverse",\n        "before",\n        "a whole board: the board before the change",\n    ),\n',''),
('ba-align-word',BM,'AlignTo::Center => "center",','AlignTo::Center => "centre",'),
('ba-align-all',BM,'        AlignTo::Middle,\n',''),
('ba-align-parse',BM,'AlignTo::ALL.iter().copied().find(|a| a.as_str() == word)','AlignTo::ALL.iter().copied().find(|a| a.as_str().eq_ignore_ascii_case(word))'),
('ba-axis-word',BM,'Axis::Vertical => "vertical",','Axis::Vertical => "down",'),
('ba-axis-all',BM,'pub const ALL: &\'static [Axis] = &[Axis::Horizontal, Axis::Vertical];','pub const ALL: &\'static [Axis] = &[Axis::Horizontal];'),
('ba-axis-parse',BM,'Axis::ALL.iter().copied().find(|a| a.as_str() == word)','Axis::ALL.iter().copied().find(|a| a.as_str().starts_with(word))'),
('ba-side-word',BM,'Side::Left => "left",','Side::Left => "west",'),
('ba-side-all',BM,'pub const ALL: &\'static [Side] = &[Side::Top, Side::Right, Side::Bottom, Side::Left];','pub const ALL: &\'static [Side] = &[Side::Top, Side::Right, Side::Bottom];'),
('ba-side-parse',BM,'Side::ALL.iter().copied().find(|s| s.as_str() == word)','Side::ALL.iter().copied().find(|s| s.as_str().eq_ignore_ascii_case(word))'),
('ba-object-id',BW,'    fields.remove("id");\n','\n'),
('ba-object-type',BW,'    fields.remove("type");\n','\n'),
('ba-removed-only',BW,'only(f, "/", &["object", "after", "frame_after"])?;',''),
('ba-removed-after-in',BW,'after: id_list(f, "after", "/after")?,','after: Vec::new(),'),
('ba-removed-frames-in',BW,'frame_after: id_list(f, "frame_after", "/frame_after")?,','frame_after: Vec::new(),'),
('ba-removed-after-out',BW,'f.insert("after".into(), ids(&self.after));',''),
('ba-removed-frames-out',BW,'f.insert("frame_after".into(), ids(&self.frame_after));',''),
('ba-id-list',BW,'Value::Str(s) => Id::new(s),','Value::Str(_) => Id::new("s-1"),'),
('ba-settings-unknown',BW,'if known(name).is_none() {','if false {'),
('ba-settings-facts',BW,'Facts::type_field("settings", Some(name)),','Facts::type_field("settings", None),'),
('ba-settings-twice',BW,'if !seen.insert(*name) {','if false {',1),
('ba-settings-null',BW,'if *text == "null" {','if false {',1),
('ba-group-refusals',BO,'    RefusalCode::IdInvalid,\n];','];'),
('ba-settings-kind',BW,'let value = by_kind(known(name), name, text, limits)?;','let value = field_from_text("settings", name, text, limits)?;'),
]
BRIDGE_SCRIPTS=[  # the scripts and tools the bridge brought: each held by its own self-test's FAILED line
('brnokit-member','scripts/check-no-kit.sh','    if BRIDGE in m:','    if False:'),
('brnokit-in-lock','scripts/check-no-kit.sh','elif p["name"] == BRIDGE:','elif False:'),
('brnokit-no-crate','scripts/check-no-kit.sh','if not seen_crate:','if False:'),
('brnokit-workspace','scripts/check-no-kit.sh',"grep -q '^\\[workspace\\]' bridge/Cargo.toml ||","true ||"),
('brlic-bridge-path','scripts/check-licences.sh','if p["source"] is None and where != "bridge":','if p["source"] is None:'),
('brcatch-under','tools/catch.py',"        if under and not path.startswith('/') and not path.startswith(under):\n            path = under + path","        if False:\n            path = under + path"),
('brcatch-tests','tools/catch.py',"TESTS = ('crates/board/tests/', 'bridge/tests/')","TESTS = ('crates/board/tests/',)"),
('brcatch-support','tools/catch.py',"SUPPORT = ('crates/board/tests/support/', 'bridge/tests/support/')","SUPPORT = ('crates/board/tests/support/',)"),
('brcatch-verdict-problems','tools/catch.py','    if problems:\n        return lines, 4\n','    if problems:\n        return lines, 0\n'),
('brcatch-verdict-none','tools/catch.py',"        return ['mutants result: none ran'], 3","        return ['mutants result: none ran'], 0"),
('brcatch-verdict-finished','tools/catch.py',"    return lines + ['mutants finished'], 0","    return lines, 0"),
('brcatch-run-lines','tools/catch.py',"    if path in mutated:\n        return mutated[path].split('\\n')","    if False:\n        return mutated[path].split('\\n')"),
('brcatch-guard','tools/catch.py',"    if not path.endswith('.rs') or old.count('\\n') == new.count('\\n'):","    if True or old.count('\\n') == new.count('\\n'):"),
('brcatch-guard-rust','tools/catch.py',"    if not path.endswith('.rs') or old.count('\\n')","    if old.count('\\n')"),
('brcatch-guard-src','tools/catch.py',"    return path.startswith(SRC) and test_module(text.split('\\n')) is not None","    return False"),
('brcatch-src','tools/catch.py',"SRC = ('crates/board/src/', 'bridge/src/')","SRC = ('crates/board/src/',)"),
]
BRIDGE_PARSE=[  # the argument parser (bridge/src/parse.rs); two take data the parser's tests read
('parse-e120-row','bridge/src/parse.rs','    usage_report("MPC-E120", d)','    usage_report("MPC-E122", d)'),
('parse-e120-hint-key','bridge/src/parse.rs','let mut d = vec![("flag", t(flag)), ("hint", t(hint))];','let mut d = vec![("flag", t(flag))];'),
('parse-e122-reason-key','bridge/src/parse.rs','let mut d = vec![("reason", t(&message)), ("verb", t(verb))];','let mut d = vec![("verb", t(verb))];'),
('parse-bad-value-apart','bridge/src/parse.rs','        ("value", t(value)),\n',''),
('parse-unknown-flag','bridge/src/parse.rs','if to.is_none() && !fields_too {','if false {'),
('parse-missing-arg','bridge/src/parse.rs','if !a.optional && a.form != Form::Fields && !g.values.contains_key(a.name) {','if false {'),
('parse-extra-positional','bridge/src/parse.rs','if positional.len() > free.len() {','if false {'),
('parse-flag-twice','bridge/src/parse.rs','if g.values.insert(a, value).is_some() {','if g.values.insert(a, value).is_some() && false {'),
('parse-double-dash','bridge/src/parse.rs','            positional.extend(tokens.by_ref());\n',''),
('parse-ignored-flag','bridge/src/parse.rs','if cmd.ignored.contains(name) {','if false {'),
('parse-alias','bridge/src/parse.rs','let word = self.aliases.get(word).map_or(word, String::as_str);','let _ = &self.aliases;'),
('parse-names-own-verb','bridge/src/parse.rs','if host != verb && args_of(host).is_some() {','if false {'),
('parse-preset-other','bridge/src/parse.rs','Some(v) if v != text => {','Some(v) if v != text && false => {'),
('parse-preset-fill','bridge/src/parse.rs','g.values.insert(arg, text.clone());','let _ = text;'),
('parse-host-order','bridge/src/parse.rs','Some(order) => order.iter().map(|(a, _)| *a).collect(),','Some(order) => order.iter().rev().map(|(a, _)| *a).collect(),'),
('parse-default-slots','bridge/src/parse.rs','.filter(|a| !a.optional && a.form != Form::Fields)\n            .map(|a| a.name)','.filter(|a| a.form != Form::Fields)\n            .map(|a| a.name)'),
('parse-meta-read','bridge/src/parse.rs','if verb == "meta" && g.values.is_empty() && g.fields.is_empty() {','if verb == "meta" && g.values.is_empty() {'),
('parse-meta-partial','bridge/src/parse.rs','if edits.set.is_empty() && edits.unset.is_empty() {','if false {'),
('parse-null-on-new','bridge/src/parse.rs','if let Some(name) = edits.unset.first() {','if let Some(name) = edits.unset.first().filter(|_| false) {'),
('parse-update-unset','bridge/src/parse.rs','        unset: edits.unset,\n','        unset: Vec::new(),\n',0),
('parse-meta-unset','bridge/src/parse.rs','                unset: edits.unset,\n','                unset: Vec::new(),\n'),
('parse-update-type','bridge/src/parse.rs','Some(o) => verbs::flags(o.kind.as_str(), &pair_refs(pairs), limits)','Some(_) => verbs::flags("sticky", &pair_refs(pairs), limits)'),
('parse-apply-phase','bridge/src/parse.rs','verb_error_report(&e, "apply", Phase::Ops)','verb_error_report(&e, "apply", Phase::Input)'),
('parse-kind-inverse','bridge/src/parse.rs','            Body::Read(_) => CallKind::Read,','            Body::Read(_) | Body::Inverse { .. } => CallKind::Read,'),
('parse-host-bug-names-verb','bridge/src/parse.rs','what: format!("{} reads the file: read it, don\'t plan it", self.name()),','what: "a read has no plan".to_string(),'),
('parse-references-both-ends','bridge/src/parse.rs','map_or(true, |i| r.from == *i || r.target == *i)','map_or(true, |i| r.from == *i)'),
('parse-number-rule','bridge/src/parse.rs','Err(r) => Err(self.bad(arg, text, "isn\'t a number", Some(&r))),','Err(_) => Ok(0.0),'),
('parse-int-whole','bridge/src/parse.rs','if n.fract() != 0.0 || n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {','if n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {'),
('parse-bool-strict','bridge/src/parse.rs','_ => Err(self.bad(arg, text, "isn\'t true or false", None)),','_ => Ok(false),'),
('parse-ids-split','bridge/src/parse.rs',"text.split(',').map(|i| self.id(arg, i)).collect()",'std::iter::once(text).map(|i| self.id(arg, i)).collect()'),
('parse-objrev-split','bridge/src/parse.rs',"let Some((id, rev)) = one.split_once('@') else {",'let Some((id, rev)) = one.split_once(\'@\').or(Some((one, "sha256-0000000000000000000000000000000000000000000000000000000000000000"))) else {'),
('parse-position-after','bridge/src/parse.rs','Some(ids) => Ok(Position::After(ids)),','Some(_) => Ok(Position::Front),'),
('parse-layer-forward','bridge/src/parse.rs','"forward" => Ok(Layer::Forward),','"forward" => Ok(Layer::Backward),'),
('parse-end-point','bridge/src/parse.rs',"if text.contains(',') {",'if false {'),
('parse-end-anchor','bridge/src/parse.rs','Some((id, a)) => (id, Some(a)),','Some((id, _)) => (id, None),'),
('parse-anchor-flag','bridge/src/parse.rs','let flag = self.anchors.get(arg).map(String::as_str);','let flag: Option<&str> = None;'),
('parse-board-word','bridge/src/parse.rs','match all.iter().copied().find(|w| name(*w) == text) {','match all.iter().copied().next() {'),
('parse-usage-optional','bridge/src/parse.rs','        out.push(format!("[--{} {}]", a.name, placeholder(a)));\n',''),
('parse-usage-json','bridge/src/parse.rs','Form::Json => Some("JSON".to_string()),','Form::Json => None,'),
('parse-example-optional','bridge/src/parse.rs','        out.push(format!("--{}", a.name));\n        out.push(sample(a, name));\n','        let _ = (a, name);\n        // no flag\n'),
('parse-host-preset-row','bridge/tests/parse_host.rs','    ("delete", "cascade", "true"),\n','    // ("delete", "cascade", "true"),\n'),
('parse-host-data-sets','crates/board/tests/data/host-cli-v1.json','"host_sets": {"delta": "1: one vote each time."}','"host_sets": {"deltas": "1: one vote each time."}'),
]
BRIDGE_RUN=[  # runs, dry runs, JSON and the file registry (bridge/src/dry_run.rs, json.rs, registry.rs)
# the file registry (tests/registry.rs)
('run-registry-drops-a-verb','bridge/src/registry.rs','.map(|&(name, _)| file_verb(name))','.skip(1).map(|&(name, _)| file_verb(name))'),
('run-registry-inverse-reads','bridge/src/registry.rs','|| name == "inverse"',''),
('run-registry-no-row-exits','bridge/src/registry.rs','exits.insert(e.exit);','let _ = e;',0),
('run-registry-no-usage-exit','bridge/src/registry.rs','for id in USAGE {','for id in USAGE.iter().take(0) {'),
('run-registry-no-zero','bridge/src/registry.rs','BTreeSet::from([0u8])','BTreeSet::new()'),
('run-registry-first-verb-only','bridge/src/registry.rs','matches!(r.way, Way::Verbs(vs) if vs.contains(&name))','matches!(r.way, Way::Verbs(vs) if vs.first() == Some(&name))'),
# a call's JSON (tests/json.rs)
('run-json-no-code','bridge/src/json.rs','out.push(("code".into(), KValue::from(report.entry().name)));',''),
('run-json-drops-details','bridge/src/json.rs','out.push((k, v));','if k != "details" {\n                        out.push((k, v));\n                    }'),
('run-json-kind','bridge/src/json.rs','("kind", KValue::from(on.kind)),','("kind", KValue::from("board")),'),
('run-json-path','bridge/src/json.rs','("path", KValue::from(on.path.as_str())),','("path", KValue::from("")),'),
('run-json-version','bridge/src/json.rs','.map_or(KValue::Null, KValue::from)','.map_or(KValue::Null, |_| KValue::Null)'),
('run-json-mode','bridge/src/json.rs','("mode", KValue::from(on.mode.as_str())),','("mode", KValue::from("live")),'),
('run-json-bug-exit','bridge/src/json.rs','out.push(("exit".into(), KValue::Null));','out.push(("exit".into(), KValue::from(1u8)));'),
('run-json-bug-says-nothing','bridge/src/json.rs','out.push(("host_bug".into(), KValue::from(bug.to_string())));','out.push(("host_bug".into(), KValue::from("")));'),
('run-json-dry-flag','bridge/src/json.rs','("dry_run", KValue::Bool(a.dry_run)),','("dry_run", KValue::Bool(false)),'),
('run-json-value','bridge/src/json.rs','("value", a.value.clone()),','("value", KValue::Null),'),
# the run and the dry run (tests/dry_run.rs)
('run-dry-says-not-dry','bridge/src/dry_run.rs','let dry = out.is_none();','let dry = false;'),
('run-dry-echoes-written','bridge/src/dry_run.rs','on: if dry { read_on } else { saved.on },','on: saved.on,'),
('run-dry-saved-key','bridge/src/dry_run.rs','if dry { "would_write" } else { "saved" },','"saved",'),
('run-read-not-dry','bridge/src/dry_run.rs','dry_run: dry,','dry_run: false,',0),
('run-saves-nowhere','bridge/src/dry_run.rs','Some(w) => w,','Some(_) => &mut sink,'),
('run-skips-apply','bridge/src/dry_run.rs','match file.apply(&planned.group) {','match file.apply(&k2_mpc_board::Group { id: planned.group.id.clone(), by: None, ops: Vec::new() }) {'),
('run-guard-after-plan','bridge/src/dry_run.rs','if now != read {','if false {'),
('run-stale-not-busy','bridge/src/dry_run.rs','return Err(refused(read_on, save_report(&stale)));','return Err(refused(read_on, io_report("stale")));'),
('run-guard-read-is-write','bridge/src/dry_run.rs','refusal_report(&r, Phase::GuardRead)','refusal_report(&r, Phase::Write)'),
('run-open-is-input','bridge/src/dry_run.rs','refusal_report(&e.value, Phase::Open)','refusal_report(&e.value, Phase::Input)'),
('run-verb-unnamed','bridge/src/dry_run.rs','change_value(call.name(),','change_value("",'),
('run-guard-seek-says-nothing','bridge/src/dry_run.rs','io_report(&e.to_string())','io_report("")',0),
('run-save-seek-says-nothing','bridge/src/dry_run.rs','io_report(&e.to_string())','io_report("")',1),
('run-registry-usage-not-the-parsers','bridge/src/registry.rs','crate::parse::usage(name)','Some(name.to_string())'),
('run-write-error-not-the-saves','bridge/src/dry_run.rs','Err(e) => return Err(refused(e.on, save_report(&e.value))),','Err(e) => return Err(refused(e.on, io_report("the save"))),'),
]
BRIDGE=BRIDGE_ROWS+BRIDGE_FACTS+BRIDGE_ARGS+BRIDGE_SCRIPTS+BRIDGE_PARSE+BRIDGE_RUN
BOTH=[
('refused-x-range-both-layers',[(J,'if past_max(&digits, exp) {','if false {'),(V,'check_value(v, &f_at, 2)?;','')],'refused-x-range'),
]
# A name, or a part of the list. The whole list can take longer than one job may run, so @part1,
# @part2 and @part3 share it out: a run stops unless they add up to the whole of it, with no overlap.
PARTS={'@format':MUT+NUM_RUST+NUM_JS+PROP+SCRIPT,'@verbs':VERB,'@both':BOTH,'@fidelity':FID,'@merge':MERGE,'@header':HEADER,
       '@bridge-rows':BRIDGE_ROWS+BRIDGE_FACTS+BRIDGE_ARGS+BRIDGE_SCRIPTS,
       '@bridge-report':BRIDGE_ROWS,'@bridge-report-a':BRIDGE_ROWS[:60],'@bridge-report-b':BRIDGE_ROWS[60:],
       '@bridge-pack':BRIDGE_FACTS+BRIDGE_ARGS+BRIDGE_SCRIPTS,'@bridge-parse':BRIDGE_PARSE,'@bridge-run':BRIDGE_RUN,'@bridge':BRIDGE}
PARTS['@part1']=PARTS['@format']
PARTS['@part2']=VERB+BOTH+FID+MERGE+HEADER
PARTS['@part3']=BRIDGE
_whole={m[0] for m in MUT+NUM_RUST+NUM_JS+PROP+SCRIPT+VERB+BOTH+FID+MERGE+HEADER+BRIDGE}
_parts=[[m[0] for m in PARTS[k]] for k in ('@part1','@part2','@part3')]
_seen=[n for part in _parts for n in part]
if len(_seen)!=len(set(_seen)) or set(_seen)!=_whole or len(_seen)!=len(_whole):
    sys.exit(f'the parts do not add up: {"+".join(str(len(p)) for p in _parts)} of {len(_whole)}')
print(f'parts: @part1={len(_parts[0])} @part2={len(_parts[1])} @part3={len(_parts[2])} whole={len(_whole)}',flush=True)
only=set()
for arg in sys.argv[1:]:
    if arg in PARTS: only|={m[0] for m in PARTS[arg]}
    else: only.add(arg)
os.environ['CARGO_TERM_COLOR']='never'
RUST_TESTS=['cargo','test','-q','--locked','--lib','--test','vectors','--test','package','--test','numbers','--test','ops','--no-fail-fast']
PROP_TESTS=['cargo','test','-q','--locked','--test','decoder_property','--no-fail-fast']
known={m[0] for m in MUT+NUM_RUST+NUM_JS+PROP+SCRIPT+VERB+BOTH+FID+MERGE+HEADER+BRIDGE}
if len(known)!=len(MUT)+len(NUM_RUST)+len(NUM_JS)+len(PROP)+len(SCRIPT)+len(VERB)+len(BOTH)+len(FID)+len(MERGE)+len(HEADER)+len(BRIDGE):
    sys.exit('two mutants share a name')
if only-known:
    sys.exit(f'no mutant by these names: {sorted(only-known)}')
# A backstop to reading catches against the mutated text: no mutant moves a test file's lines.
_moved=[m[0] for m in MUT+NUM_RUST+NUM_JS+PROP+SCRIPT+VERB+FID+MERGE+HEADER+BRIDGE if catch.moves_test_lines(m[1],open(m[1],encoding='utf8').read(),m[2],m[3])]
_moved+=[name for name,edits,want in BOTH for path,old,new in edits if catch.moves_test_lines(path,open(path,encoding='utf8').read(),old,new)]
if _moved:
    sys.exit(f'mutants that change the line count of a file with test code (keep it): {_moved}')
VERB_TESTS=['cargo','test','-q','--locked','--test','file','--test','verbs','--test','arrange','--test','parity','--test','merge','--test','host_cli','--no-fail-fast']
MERGE_TESTS=['cargo','test','-q','--locked','--test','merge','--no-fail-fast']
HEADER_TESTS=['cargo','test','-q','--locked','--test','header','--no-fail-fast']
FID_TESTS=['cargo','test','-q','--locked','--test','fidelity','--no-fail-fast']
VECTOR_TEST=['cargo','test','-q','--locked','--test','vectors','every_vector_holds','--no-fail-fast']
JS_TESTS=['node','vectors/check.mjs']
SCRIPT_TESTS=['sh','scripts/test-host-forms.sh']
LICENCE_TESTS=['sh','scripts/test-licences.sh']
CREDIT_CHECK=['sh','scripts/check-credit.sh']
CREDIT_TESTS=['sh','scripts/test-credit.sh']
CATCH_TESTS=['python3','-I','tools/test_catch.py']
BRIDGE_TESTS=['cargo','test','-q','--locked','--manifest-path','bridge/Cargo.toml','--no-fail-fast']
FACTS_TESTS=['cargo','test','-q','--locked','--test','facts','--no-fail-fast']
NOKIT_TESTS=['sh','scripts/test-no-kit.sh']
ARGS_TESTS=['cargo','test','-q','--locked','--test','args','--test','group_refusals','--lib','--no-fail-fast']
def attempt(cmd):
    p=subprocess.run(cmd,capture_output=True,text=True,timeout=600)
    return p.returncode,p.stdout+p.stderr
def source(path):
    # Read after the mutant's edit is put back, so it's the run's own commit.
    try: return open(path,encoding='utf8').read().split('\n')
    except OSError: return []
def kinds(out,under='',mutated={}):
    # Every failure, in order, with the kind that counts there or why it doesn't. `under`: the bridge's
    # workspace folder, whose paths cargo prints from there. `mutated`: the mutated files' text by path,
    # kept from before the restore, which is what the run's lines are.
    return [(t,f'{p}:{l}:{c}')+catch.classify(p,l,c,catch.run_lines(p,mutated,source)) for t,p,l,c in catch.locations(out,under)]
def assertion(out,rust=True,under='',mutated={}):
    lines=out.split('\n')
    if not rust:
        for l in lines:
            if catch.script_case(l): return '[script] '+l[:230]
        return None
    for test,where,kind,why in kinds(out,under,mutated):
        if not kind: continue
        shown=where[len(under):] if under and where.startswith(under) else where
        at=next(i for i,l in enumerate(lines) if f' panicked at {shown}:' in l or f' panicked at {where}:' in l)
        msg=' '.join(x.strip() for x in lines[at+1:at+6] if x.strip() and not x.startswith('note:'))[:230]
        return f'[{kind}] {where} {msg}'
    return None
def print_failing(name,out,under='',mutated={}):
    # Every test that failed and where, with what counts there: one line each, read test by test.
    for test,where,kind,why in kinds(out,under,mutated):
        print(f'    FAILS {name} {test} {where} '+(f'[{kind}]' if kind else f'(not counted: {why})'),flush=True)
c=dict(caught=0,survived=0,equivalent=0,stale=0,build_failed=0,no_assert=0,aborts_expected=0)
problems=[]
for name,cmd in (('rust',RUST_TESTS),('property',PROP_TESTS),('verbs',VERB_TESTS),('fidelity',FID_TESTS),('merge',MERGE_TESTS),('header',HEADER_TESTS),('js',JS_TESTS),('scripts',SCRIPT_TESTS),('catch',CATCH_TESTS),('credit',CREDIT_CHECK),('credit-self',CREDIT_TESTS),('bridge',BRIDGE_TESTS),('facts',FACTS_TESTS),('no-kit',NOKIT_TESTS),('args',ARGS_TESTS),('licences',LICENCE_TESTS)):
    rc,out=attempt(cmd)
    print(f'control {name} exit {rc}',flush=True)
    if rc!=0:
        print(out[-2000:]); sys.exit(2)
def sweep(items,cmd,build,under=''):
    for m in items:
        name,path,old,new=m[:4]; nth=m[4] if len(m)>4 else None
        if only and name not in only: continue
        src=open(path,encoding='utf8').read()
        n=src.count(old)
        if n==0 or (nth is None and n!=1) or (nth is not None and nth>=n):
            print(f'MUTANT={name} STALE: pattern found {n} times',flush=True); c['stale']+=1; problems.append(name); continue
        if nth is None: mutated=src.replace(old,new,1)
        else:
            idx=-1
            for _ in range(nth+1): idx=src.index(old,idx+1)
            mutated=src[:idx]+new+src[idx+len(old):]
        open(path,'w',encoding='utf8').write(mutated)
        print(f'MUTANT={name} applied to {path}',flush=True)
        try:
            if build:
                rc,out=attempt(cmd+['--no-run'])
                if rc!=0:
                    e=[l for l in out.split('\n') if l.startswith('error')][:2]
                    print(f'  BUILD-FAILED {name}: {e}',flush=True); c['build_failed']+=1; problems.append(name); continue
            try: rc,out=attempt(cmd)
            except subprocess.TimeoutExpired: rc,out=124,'timed out'
        finally:
            open(path,'w',encoding='utf8').write(src)
        if rc==0:
            if name in EQUIVALENT:
                print(f'  EQUIVALENT {name}: {EQUIVALENT[name]}',flush=True); c['equivalent']+=1
            else:
                print(f'  SURVIVED {name}',flush=True); c['survived']+=1; problems.append(name)
            continue
        hit=assertion(out,build,under,{path:mutated})
        if hit:
            print(f'  CAUGHT {name}: {hit}',flush=True); c['caught']+=1; print_failing(name,out,under,{path:mutated}); continue
        sig=re.search(r'signal: (\d+)[^)]*',out)
        if sig and name in EXPECT_ABORT:
            print(f'  ABORT-EXPECTED {name}: {sig.group(0)}: {EXPECT_ABORT[name]}',flush=True); c['aborts_expected']+=1; continue
        tail=[l for l in out.split('\n') if l.strip()][-3:]
        print(f'  NO-ASSERT {name}: rc={rc} {tail}',flush=True); c['no_assert']+=1; problems.append(name); print_failing(name,out,under,{path:mutated})
def sweep_both(items,cmd):
    for name,edits,want in items:
        if only and name not in only: continue
        sources={path:open(path,encoding='utf8').read() for path,_,_ in edits}
        counts=[sources[path].count(old) for path,old,_ in edits]
        if any(n!=1 for n in counts):
            print(f'MUTANT={name} STALE: patterns found {counts} times',flush=True); c['stale']+=1; problems.append(name); continue
        muts={}
        for path,old,new in edits:
            sources_now=open(path,encoding='utf8').read()
            open(path,'w',encoding='utf8').write(sources_now.replace(old,new,1))
            muts[path]=sources_now.replace(old,new,1)
        print(f'MUTANT={name} applied to {sorted(sources)}, wanting vector {want}',flush=True)
        try:
            rc,out=attempt(cmd+['--no-run'])
            if rc!=0:
                e=[l for l in out.split('\n') if l.startswith('error')][:2]
                print(f'  BUILD-FAILED {name}: {e}',flush=True); c['build_failed']+=1; problems.append(name); continue
            try: rc,out=attempt(cmd)
            except subprocess.TimeoutExpired: rc,out=124,'timed out'
        finally:
            for path,src in sources.items(): open(path,'w',encoding='utf8').write(src)
        if rc==0:
            print(f'  SURVIVED {name}',flush=True); c['survived']+=1; problems.append(name); continue
        hit=assertion(out,True,'',muts)
        at=out.find(f'"{want}"')
        if hit and at>=0:
            print(f'  CAUGHT {name}: {hit[:80]} ... '+' '.join(out[at:at+90].split()),flush=True); c['caught']+=1
        else:
            print(f'  NO-ASSERT {name}: rc={rc}, {want} isn\'t in the failure text',flush=True); c['no_assert']+=1; problems.append(name)
sweep(MUT,RUST_TESTS,True)
sweep(NUM_RUST,RUST_TESTS,True)
sweep(NUM_JS,JS_TESTS,False)
sweep(PROP,PROP_TESTS,True)
sweep(VERB,VERB_TESTS,True)
sweep_both(BOTH,VECTOR_TEST)
sweep(FID,FID_TESTS,True)
sweep(MERGE,MERGE_TESTS,True)
sweep(HEADER,HEADER_TESTS,True)
sweep(BRIDGE_ROWS,BRIDGE_TESTS,True,'bridge/')
sweep(BRIDGE_FACTS,FACTS_TESTS,True)
sweep(BRIDGE_ARGS,ARGS_TESTS,True)
sweep([m for m in BRIDGE_SCRIPTS if m[0].startswith('brnokit-')],NOKIT_TESTS,False)
sweep([m for m in BRIDGE_SCRIPTS if m[0].startswith('brlic-')],LICENCE_TESTS,False)
sweep([m for m in BRIDGE_SCRIPTS if m[0].startswith('brcatch-')],CATCH_TESTS,False)
sweep(BRIDGE_PARSE,BRIDGE_TESTS,True,'bridge/')
sweep(BRIDGE_RUN,BRIDGE_TESTS,True,'bridge/')
sweep([m for m in SCRIPT if m[0].startswith('forms-')],SCRIPT_TESTS,False)
sweep([m for m in SCRIPT if m[0].startswith('licences-')],LICENCE_TESTS,False)
sweep([m for m in SCRIPT if m[0].startswith('catch-')],CATCH_TESTS,False)
sweep([m for m in SCRIPT if m[0].startswith('credit-tree-')],CREDIT_CHECK,False)
sweep([m for m in SCRIPT if m[0].startswith('credit-self-')],CREDIT_TESTS,False)
lines,code=catch.verdict(c,problems)
for l in lines: print(l,flush=True)
sys.exit(code)
