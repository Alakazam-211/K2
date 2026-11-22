//! The query-string grammar (PRD §7.2): `select`, filters, `or`/`and`,
//! `order`, `limit`/`offset`, `columns`, `on_conflict`, and the embed paths
//! on each (`items.order=…`). Parts PostgREST has and v1 doesn't are
//! refused with [`ApiError::unsupported`] here, never guessed at.

use crate::error::ApiError;
use crate::sql::is_type_name;

/// `->` (JSON) or `->>` (text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Json,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonKey {
    Name(String),
    Index(i64),
}

/// A column, maybe with a JSON path: `data->a->>b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub path: Vec<(Arrow, JsonKey)>,
}

impl Field {
    /// The name PostgREST gives it in an answer: the last JSON key, else
    /// the column.
    pub fn label(&self) -> String {
        match self.path.last() {
            Some((_, JsonKey::Name(k))) => k.clone(),
            Some((_, JsonKey::Index(i))) => i.to_string(),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Default,
    Inner,
    Left,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embed {
    /// The table, or a foreign key column of the parent.
    pub name: String,
    pub alias: Option<String>,
    /// `!fk_name` or `!column`.
    pub hint: Option<String>,
    pub join: JoinKind,
    /// Empty `rel()`: filter only, nothing shown.
    pub select: Vec<SelectItem>,
}

impl Embed {
    /// How filters and the answer name it: the alias, else the name.
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectItem {
    Star,
    Col { field: Field, alias: Option<String>, cast: Option<String> },
    Embed(Embed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    Ilike,
    Match,
    Imatch,
    In,
    Is,
    Cs,
    Cd,
}

impl Op {
    fn parse(s: &str) -> Result<Op, ApiError> {
        Ok(match s {
            "eq" => Op::Eq,
            "neq" => Op::Neq,
            "gt" => Op::Gt,
            "gte" => Op::Gte,
            "lt" => Op::Lt,
            "lte" => Op::Lte,
            "like" => Op::Like,
            "ilike" => Op::Ilike,
            "match" => Op::Match,
            "imatch" => Op::Imatch,
            "in" => Op::In,
            "is" => Op::Is,
            "cs" => Op::Cs,
            "cd" => Op::Cd,
            "fts" | "plfts" | "phfts" | "wfts" => return Err(ApiError::unsupported("full-text search (textSearch)")),
            "ov" => return Err(ApiError::unsupported("the overlaps filter (ov)")),
            "sl" | "sr" | "nxl" | "nxr" | "adj" => return Err(ApiError::unsupported("the range operators (sl, sr, nxl, nxr, adj)")),
            "isdistinct" => return Err(ApiError::unsupported("the isdistinct filter")),
            "all" | "any" => return Err(ApiError::unsupported("filter quantifiers (any, all)")),
            _ => return Err(ApiError::parse(format!("\"failed to parse filter ({s})\""), format!("unknown operator {s}"))),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsVal {
    Null,
    True,
    False,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    One(String),
    List(Vec<String>),
    Is(IsVal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cond {
    /// `field=[not.]op.value`
    Op { field: Field, negated: bool, op: Op, value: Operand },
    /// `or=(…)` / `and=(…)` (one level), maybe `not.`.
    Logic { negated: bool, and: bool, children: Vec<Cond> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nulls {
    First,
    Last,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderTerm {
    pub field: Field,
    pub dir: Option<Dir>,
    pub nulls: Option<Nulls>,
}

/// A filter on an embed itself: `rel=is.null` (no related row) or
/// `rel=not.is.null`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedNull {
    pub embed_path: Vec<String>,
    pub is_null: bool,
}

/// Everything the query string says, by embed path (`[]` is the root).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub select: Option<Vec<SelectItem>>,
    pub filters: Vec<(Vec<String>, Cond)>,
    /// `key=value` pairs that may be filters or (GET rpc) arguments,
    /// decided once the function is known.
    pub raw: Vec<(Vec<String>, String, String)>,
    pub order: Vec<(Vec<String>, Vec<OrderTerm>)>,
    pub limit: Vec<(Vec<String>, u64)>,
    pub offset: Vec<(Vec<String>, u64)>,
    pub columns: Option<Vec<String>>,
    pub on_conflict: Option<Vec<String>>,
}

impl Query {
    pub fn filters_at<'a>(&'a self, path: &'a [String]) -> impl Iterator<Item = &'a Cond> + 'a {
        self.filters.iter().filter(move |(p, _)| p == path).map(|(_, c)| c)
    }
    pub fn order_at(&self, path: &[String]) -> Option<&Vec<OrderTerm>> {
        self.order.iter().rev().find(|(p, _)| p == path).map(|(_, o)| o)
    }
    pub fn limit_at(&self, path: &[String]) -> Option<u64> {
        self.limit.iter().rev().find(|(p, _)| p == path).map(|(_, n)| *n)
    }
    pub fn offset_at(&self, path: &[String]) -> Option<u64> {
        self.offset.iter().rev().find(|(p, _)| p == path).map(|(_, n)| *n)
    }
}

/// Parse the decoded query pairs. Filters stay in `raw` until
/// [`Query::take_filters`] turns them into conditions (a GET rpc first
/// takes its arguments out).
pub fn parse_query(pairs: &[(String, String)]) -> Result<Query, ApiError> {
    let mut q = Query::default();
    for (k, v) in pairs {
        crate::sql::check_value(k)?;
        crate::sql::check_value(v)?;
        let (path, last) = split_path(k);
        match last.as_str() {
            "select" if path.is_empty() => q.select = Some(parse_select(v)?),
            "order" => q.order.push((path, parse_order(v)?)),
            "limit" => q.limit.push((path, parse_count(v, "limit")?)),
            "offset" => q.offset.push((path, parse_count(v, "offset")?)),
            "columns" if path.is_empty() => q.columns = Some(parse_name_list(v)?),
            "on_conflict" if path.is_empty() => q.on_conflict = Some(parse_name_list(v)?),
            "or" | "and" | "not.or" | "not.and" => {
                let negated = last.starts_with("not.");
                let and = last.ends_with("and");
                q.filters.push((path, parse_logic(v, negated, and)?));
            }
            _ => q.raw.push((path, last, v.clone())),
        }
    }
    Ok(q)
}

impl Query {
    /// Turn every `raw` pair into a filter.
    pub fn take_filters(&mut self) -> Result<(), ApiError> {
        for (path, k, v) in std::mem::take(&mut self.raw) {
            let field = parse_field(&k)?;
            self.filters.push((path, parse_filter(field, &v)?));
        }
        Ok(())
    }
}

/// `items.sub.status` → (`[items, sub]`, `status`); `not.or` stays whole.
fn split_path(k: &str) -> (Vec<String>, String) {
    for tail in ["not.or", "not.and"] {
        if k == tail {
            return (Vec::new(), k.to_string());
        }
        if let Some(head) = k.strip_suffix(&format!(".{tail}")) {
            return (head.split('.').map(str::to_string).collect(), tail.to_string());
        }
    }
    // A JSON path never holds a dot, so the last dot ends the embed path.
    match k.rsplit_once('.') {
        Some((head, last)) if !head.contains("->") => (head.split('.').map(str::to_string).collect(), last.to_string()),
        _ => (Vec::new(), k.to_string()),
    }
}

fn parse_count(v: &str, what: &str) -> Result<u64, ApiError> {
    v.trim().parse::<u64>().map_err(|_| ApiError::parse(format!("\"failed to parse {what} parameter ({v})\""), format!("{what} must be a non-negative integer")))
}

/// `a,b` or `"a","b c"`.
fn parse_name_list(v: &str) -> Result<Vec<String>, ApiError> {
    split_top(v)?
        .into_iter()
        .map(|s| {
            let s = s.trim();
            let n = unquote(s);
            if n.is_empty() {
                Err(ApiError::parse(format!("\"failed to parse columns parameter ({v})\""), "empty name"))
            } else {
                Ok(n)
            }
        })
        .collect()
}

fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        s.to_string()
    }
}

/// Split on commas outside parentheses, braces and double quotes.
fn split_top(s: &str) -> Result<Vec<String>, ApiError> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut brace, mut quoted) = (0i32, 0i32, false);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                continue;
            }
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            '{' if !quoted => brace += 1,
            '}' if !quoted => brace -= 1,
            ',' if !quoted && depth == 0 && brace == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        if depth < 0 {
            return Err(ApiError::parse(format!("\"failed to parse ({s})\""), "unbalanced parentheses"));
        }
        cur.push(c);
    }
    if depth != 0 || quoted {
        return Err(ApiError::parse(format!("\"failed to parse ({s})\""), "unbalanced parentheses or quotes"));
    }
    out.push(cur);
    Ok(out)
}

/// A column with an optional JSON path.
pub fn parse_field(s: &str) -> Result<Field, ApiError> {
    let s = s.trim();
    let mut parts = Vec::new();
    let mut rest = s;
    let name_end = rest.find("->").unwrap_or(rest.len());
    let name = unquote(rest[..name_end].trim());
    rest = &rest[name_end..];
    while !rest.is_empty() {
        let (arrow, after) = if let Some(a) = rest.strip_prefix("->>") {
            (Arrow::Text, a)
        } else if let Some(a) = rest.strip_prefix("->") {
            (Arrow::Json, a)
        } else {
            return Err(ApiError::parse(format!("\"failed to parse ({s})\""), "bad JSON path"));
        };
        let end = after.find("->").unwrap_or(after.len());
        let key = after[..end].trim();
        if key.is_empty() {
            return Err(ApiError::parse(format!("\"failed to parse ({s})\""), "empty JSON key"));
        }
        let key = match key.parse::<i64>() {
            Ok(i) => JsonKey::Index(i),
            Err(_) => JsonKey::Name(unquote(key)),
        };
        parts.push((arrow, key));
        rest = &after[end..];
    }
    if name.is_empty() || name.contains(['(', ')', ',', ':', '!']) {
        return Err(ApiError::parse(format!("\"failed to parse ({s})\""), "bad column name"));
    }
    Ok(Field { name, path: parts })
}

/// `[not.]op.value`, the right side of a filter.
pub fn parse_filter(field: Field, v: &str) -> Result<Cond, ApiError> {
    let (negated, rest) = match v.strip_prefix("not.") {
        Some(r) => (true, r),
        None => (false, v),
    };
    let (op, value) = rest.split_once('.').ok_or_else(|| ApiError::parse(format!("\"failed to parse filter ({v})\""), "a filter is op.value"))?;
    if op.contains('(') {
        return Err(ApiError::unsupported("filter quantifiers (eq(any), like(all), …)"));
    }
    let op = Op::parse(op)?;
    let value = operand(op, value, v)?;
    Ok(Cond::Op { field, negated, op, value })
}

fn operand(op: Op, value: &str, whole: &str) -> Result<Operand, ApiError> {
    Ok(match op {
        Op::In => {
            let inner = value
                .strip_prefix('(')
                .and_then(|x| x.strip_suffix(')'))
                .ok_or_else(|| ApiError::parse(format!("\"failed to parse filter ({whole})\""), "in takes a list: in.(a,b)"))?;
            if inner.trim().is_empty() {
                Operand::List(Vec::new())
            } else {
                Operand::List(split_top(inner)?.into_iter().map(|s| unquote(s.trim())).collect())
            }
        }
        Op::Is => Operand::Is(match value.to_ascii_lowercase().as_str() {
            "null" => IsVal::Null,
            "true" => IsVal::True,
            "false" => IsVal::False,
            "unknown" => IsVal::Unknown,
            _ => return Err(ApiError::parse(format!("\"failed to parse filter ({whole})\""), "is takes null, true, false or unknown")),
        }),
        Op::Like | Op::Ilike => Operand::One(unquote(value).replace('*', "%")),
        _ => Operand::One(unquote(value)),
    })
}

/// `(a.eq.1,b.in.(1,2),c.not.is.null)`, one level.
fn parse_logic(v: &str, negated: bool, and: bool) -> Result<Cond, ApiError> {
    let inner = v
        .trim()
        .strip_prefix('(')
        .and_then(|x| x.strip_suffix(')'))
        .ok_or_else(|| ApiError::parse(format!("\"failed to parse logic tree ({v})\""), "or and and take (…)"))?;
    let mut children = Vec::new();
    for piece in split_top(inner)? {
        let piece = piece.trim();
        let bare = piece.strip_prefix("not.").unwrap_or(piece);
        if bare.starts_with("and(") || bare.starts_with("or(") {
            return Err(ApiError::unsupported("a nested and()/or() inside or()"));
        }
        let (f, rest) = piece.split_once('.').ok_or_else(|| ApiError::parse(format!("\"failed to parse logic tree ({v})\""), format!("bad piece {piece}")))?;
        children.push(parse_filter(parse_field(f)?, rest)?);
    }
    Ok(Cond::Logic { negated, and, children })
}

/// `a.desc.nullslast,b`
fn parse_order(v: &str) -> Result<Vec<OrderTerm>, ApiError> {
    let mut out = Vec::new();
    for term in split_top(v)? {
        let term = term.trim();
        if term.contains('(') {
            return Err(ApiError::unsupported("ordering by an embedded resource's column"));
        }
        let mut parts: Vec<&str> = term.split('.').collect();
        let mut nulls = None;
        let mut dir = None;
        if let Some(last) = parts.last() {
            match *last {
                "nullsfirst" => nulls = Some(Nulls::First),
                "nullslast" => nulls = Some(Nulls::Last),
                _ => {}
            }
        }
        if nulls.is_some() {
            parts.pop();
        }
        if let Some(last) = parts.last() {
            match *last {
                "asc" => dir = Some(Dir::Asc),
                "desc" => dir = Some(Dir::Desc),
                _ => {}
            }
        }
        if dir.is_some() {
            parts.pop();
        }
        if parts.len() != 1 {
            return Err(ApiError::parse(format!("\"failed to parse order ({v})\""), format!("bad order term {term}")));
        }
        out.push(OrderTerm { field: parse_field(parts[0])?, dir, nulls });
    }
    Ok(out)
}

// ── select ───────────────────────────────────────────────────────────

/// `*,alias:col::text,rel!fk!inner(a,b),data->>x`
pub fn parse_select(v: &str) -> Result<Vec<SelectItem>, ApiError> {
    let v = v.trim();
    if v.is_empty() {
        return Ok(vec![SelectItem::Star]);
    }
    split_top(v)?.into_iter().map(|s| parse_item(s.trim(), v)).collect()
}

fn parse_item(s: &str, whole: &str) -> Result<SelectItem, ApiError> {
    let bad = |why: &str| ApiError::parse(format!("\"failed to parse select parameter ({whole})\""), why.to_string());
    if s.is_empty() {
        return Err(bad("empty item"));
    }
    if s == "*" {
        return Ok(SelectItem::Star);
    }
    if s.starts_with("...") {
        return Err(ApiError::unsupported("spread embeds (...rel)"));
    }
    // An embed: name[!hint][!inner](…)
    if let Some(open) = s.find('(') {
        if !s.ends_with(')') {
            return Err(bad("an embed ends with )"));
        }
        let head = &s[..open];
        let inner = &s[open + 1..s.len() - 1];
        if head.ends_with(".count") || head.ends_with(".sum") || head.ends_with(".avg") || head.ends_with(".min") || head.ends_with(".max") || head == "count" {
            return Err(ApiError::unsupported("aggregate functions in select"));
        }
        let (alias, head) = split_alias(head);
        let mut bits = head.split('!');
        let name = bits.next().unwrap_or("").trim().to_string();
        if name.is_empty() || name.contains("->") {
            return Err(bad("bad embed name"));
        }
        let (mut hint, mut join) = (None, JoinKind::Default);
        for b in bits {
            match b.trim() {
                "inner" => join = JoinKind::Inner,
                "left" => join = JoinKind::Left,
                "" => return Err(bad("empty hint")),
                h => {
                    if hint.is_some() {
                        return Err(bad("two hints"));
                    }
                    hint = Some(h.to_string());
                }
            }
        }
        let select = if inner.trim().is_empty() { Vec::new() } else { parse_select(inner)? };
        return Ok(SelectItem::Embed(Embed { name, alias, hint, join, select }));
    }
    let (alias, rest) = split_alias(s);
    let (field_s, cast) = match rest.split_once("::") {
        Some((f, c)) => {
            let c = c.trim();
            if !is_type_name(c) {
                return Err(bad("bad cast type"));
            }
            (f, Some(c.to_string()))
        }
        None => (rest, None),
    };
    if field_s.contains('.') && !field_s.contains("->") {
        return Err(ApiError::unsupported("aggregates or a dotted column in select"));
    }
    Ok(SelectItem::Col { field: parse_field(field_s)?, alias, cast })
}

/// `alias:rest` (a `::` cast is not an alias).
fn split_alias(s: &str) -> (Option<String>, &str) {
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b':' {
            if i + 1 < b.len() && b[i + 1] == b':' {
                return (None, s);
            }
            let a = s[..i].trim();
            if a.is_empty() || a.contains("->") {
                return (None, s);
            }
            return (Some(unquote(a)), &s[i + 1..]);
        }
        if b[i] == b'(' || b[i] == b'-' {
            return (None, s);
        }
        i += 1;
    }
    (None, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(q: &[(&str, &str)]) -> Vec<(String, String)> {
        q.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn select_items_parse() {
        let s = parse_select("*,label:name,id::text,data->a->>b,owner:profiles!notes_owner_fkey!inner(id,name),items(*),tags()").unwrap();
        assert_eq!(s[0], SelectItem::Star);
        assert_eq!(s[1], SelectItem::Col { field: Field { name: "name".into(), path: vec![] }, alias: Some("label".into()), cast: None });
        assert_eq!(s[2], SelectItem::Col { field: Field { name: "id".into(), path: vec![] }, alias: None, cast: Some("text".into()) });
        match &s[3] {
            SelectItem::Col { field, .. } => {
                assert_eq!(field.path, vec![(Arrow::Json, JsonKey::Name("a".into())), (Arrow::Text, JsonKey::Name("b".into()))]);
                assert_eq!(field.label(), "b");
            }
            o => panic!("{o:?}"),
        }
        match &s[4] {
            SelectItem::Embed(e) => {
                assert_eq!((e.name.as_str(), e.alias.as_deref(), e.hint.as_deref(), e.join), ("profiles", Some("owner"), Some("notes_owner_fkey"), JoinKind::Inner));
                assert_eq!(e.select.len(), 2);
                assert_eq!(e.key(), "owner");
            }
            o => panic!("{o:?}"),
        }
        match &s[6] {
            SelectItem::Embed(e) => assert!(e.select.is_empty()),
            o => panic!("{o:?}"),
        }
        assert_eq!(parse_select("...profiles(name)").unwrap_err().code, "K2REST001");
        assert_eq!(parse_select("id.count()").unwrap_err().code, "K2REST001");
        assert_eq!(parse_select("id::int) union select (1").unwrap_err().code, "PGRST100");
        assert_eq!(parse_select("a,(b").unwrap_err().code, "PGRST100");
    }

    #[test]
    fn filters_or_order_and_paths_parse() {
        let mut q = parse_query(&pairs(&[
            ("id", "eq.5"),
            ("name", "not.ilike.*ann*"),
            ("status", "in.(open,\"a,b\",closed)"),
            ("deleted_at", "is.null"),
            ("tags", "cs.{a,b}"),
            ("data->>kind", "eq.x.y"),
            ("or", "(a.eq.1,b.not.in.(1,2),c.is.true)"),
            ("items.status", "eq.done"),
            ("items.or", "(x.eq.1,y.eq.2)"),
            ("order", "created_at.desc.nullslast,id"),
            ("items.order", "pos.asc"),
            ("items.limit", "3"),
            ("limit", "10"),
            ("offset", "20"),
            ("columns", "\"a\",\"b\""),
            ("on_conflict", "id"),
        ]))
        .unwrap();
        q.take_filters().unwrap();
        let root: Vec<&Cond> = q.filters_at(&[]).collect();
        assert!(root.contains(&&Cond::Op { field: Field { name: "id".into(), path: vec![] }, negated: false, op: Op::Eq, value: Operand::One("5".into()) }));
        assert!(root.contains(&&Cond::Op { field: Field { name: "name".into(), path: vec![] }, negated: true, op: Op::Ilike, value: Operand::One("%ann%".into()) }));
        assert!(root.contains(&&Cond::Op {
            field: Field { name: "status".into(), path: vec![] },
            negated: false,
            op: Op::In,
            value: Operand::List(vec!["open".into(), "a,b".into(), "closed".into()])
        }));
        assert!(root.contains(&&Cond::Op { field: Field { name: "deleted_at".into(), path: vec![] }, negated: false, op: Op::Is, value: Operand::Is(IsVal::Null) }));
        assert!(root.contains(&&Cond::Op {
            field: Field { name: "data".into(), path: vec![(Arrow::Text, JsonKey::Name("kind".into()))] },
            negated: false,
            op: Op::Eq,
            value: Operand::One("x.y".into())
        }));
        match root.iter().find(|c| matches!(c, Cond::Logic { .. })).unwrap() {
            Cond::Logic { negated, and, children } => {
                assert!(!negated && !and);
                assert_eq!(children.len(), 3);
            }
            _ => unreachable!(),
        }
        let items = vec!["items".to_string()];
        assert_eq!(q.filters_at(&items).count(), 2);
        assert_eq!(q.limit_at(&items), Some(3));
        assert_eq!(q.limit_at(&[]), Some(10));
        assert_eq!(q.offset_at(&[]), Some(20));
        let o = q.order_at(&[]).unwrap();
        assert_eq!((o[0].dir, o[0].nulls), (Some(Dir::Desc), Some(Nulls::Last)));
        assert_eq!((o[1].dir, o[1].nulls), (None, None));
        assert_eq!(q.columns, Some(vec!["a".into(), "b".into()]));
        assert_eq!(q.on_conflict, Some(vec!["id".into()]));
    }

    #[test]
    fn unsupported_parts_are_named_not_guessed() {
        for (k, v) in [("title", "fts.cat"), ("r", "ov.{1,2}"), ("r", "sl.[1,2]"), ("a", "eq(any).{1,2}")] {
            let mut q = parse_query(&pairs(&[(k, v)])).unwrap();
            assert_eq!(q.take_filters().unwrap_err().code, "K2REST001", "{k}={v}");
        }
        assert_eq!(parse_query(&pairs(&[("or", "(a.eq.1,and(b.eq.1,c.eq.2))")])).unwrap_err().code, "K2REST001");
        let mut q = parse_query(&pairs(&[("a", "nope.1")])).unwrap();
        assert_eq!(q.take_filters().unwrap_err().code, "PGRST100");
        assert_eq!(parse_query(&pairs(&[("order", "author(name).asc")])).unwrap_err().code, "K2REST001");
        assert_eq!(parse_query(&pairs(&[("limit", "x")])).unwrap_err().code, "PGRST100");
    }
}
