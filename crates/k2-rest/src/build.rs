//! From a parsed request to SQL (PRD §7.1): one generated statement per
//! request (two for a planned count), every name quoted, every value a
//! literal. Rows become JSON inside Postgres, as PostgREST does.
//!
//! The main statement of a read, an rpc or a write that returns its rows
//! answers exactly one row of three text columns: `total` (the count asked
//! for, else NULL), `page_total` (rows in this answer) and `body` (a JSON
//! array, or for a scalar function its JSON value).

use serde_json::{Map, Value};

use crate::error::ApiError;
use crate::parse::{Arrow, Cond, Dir, Embed, Field, IsVal, JoinKind, JsonKey, Nulls, Op, Operand, OrderTerm, Query, SelectItem};
use crate::schema::{Function, SchemaCache, Table};
use crate::sql::{lit, qi};

/// One relation the builder reads columns of.
#[derive(Clone)]
pub(crate) struct Rel<'a> {
    /// The table (for columns and relationships), when there is one.
    pub table: Option<&'a Table>,
    /// How SQL names its columns: `"public"."notes"`, `"pgrst_3"`,
    /// `"pgrst_source"`.
    pub rref: String,
}

pub(crate) struct Builder<'a> {
    pub cache: &'a SchemaCache,
    pub schema: String,
    pub q: &'a Query,
    n: usize,
}

/// One relationship an embed resolved to.
struct RelShip<'a> {
    target: &'a Table,
    to_one: bool,
    /// (child column, parent column) pairs.
    pairs: Vec<(String, String)>,
}

pub(crate) fn field_sql(rref: &str, f: &Field) -> String {
    let mut s = format!("{rref}.{}", qi(&f.name));
    for (a, k) in &f.path {
        s.push_str(match a {
            Arrow::Json => "->",
            Arrow::Text => "->>",
        });
        match k {
            JsonKey::Name(n) => s.push_str(&lit(n)),
            JsonKey::Index(i) => s.push_str(&i.to_string()),
        }
    }
    s
}

pub(crate) fn cond_sql(rref: &str, c: &Cond) -> Result<String, ApiError> {
    Ok(match c {
        Cond::Op { field, negated, op, value } => {
            let lhs = field_sql(rref, field);
            let bin = |o: &str, v: &Operand| -> Result<String, ApiError> {
                match v {
                    Operand::One(x) => Ok(format!("{lhs} {o} {}", lit(x))),
                    _ => Err(ApiError::parse("\"failed to parse filter\"", "bad operand")),
                }
            };
            let e = match op {
                Op::Eq => bin("=", value)?,
                Op::Neq => bin("<>", value)?,
                Op::Gt => bin(">", value)?,
                Op::Gte => bin(">=", value)?,
                Op::Lt => bin("<", value)?,
                Op::Lte => bin("<=", value)?,
                Op::Like => bin("LIKE", value)?,
                Op::Ilike => bin("ILIKE", value)?,
                Op::Match => bin("~", value)?,
                Op::Imatch => bin("~*", value)?,
                Op::Cs => bin("@>", value)?,
                Op::Cd => bin("<@", value)?,
                Op::In => match value {
                    Operand::List(vs) if vs.is_empty() => "false".to_string(),
                    // Untyped literals: Postgres types them from the column.
                    Operand::List(vs) => format!("{lhs} IN ({})", vs.iter().map(|v| lit(v)).collect::<Vec<_>>().join(", ")),
                    _ => return Err(ApiError::parse("\"failed to parse filter\"", "in takes a list")),
                },
                Op::Is => match value {
                    Operand::Is(v) => format!(
                        "{lhs} IS {}",
                        match v {
                            IsVal::Null => "NULL",
                            IsVal::True => "TRUE",
                            IsVal::False => "FALSE",
                            IsVal::Unknown => "UNKNOWN",
                        }
                    ),
                    _ => return Err(ApiError::parse("\"failed to parse filter\"", "is takes null, true, false or unknown")),
                },
            };
            if *negated {
                format!("NOT ({e})")
            } else {
                e
            }
        }
        Cond::Logic { negated, and, children } => {
            let parts = children.iter().map(|c| cond_sql(rref, c)).collect::<Result<Vec<_>, _>>()?;
            let joined = if parts.is_empty() { (if *and { "true" } else { "false" }).to_string() } else { parts.join(if *and { " AND " } else { " OR " }) };
            if *negated {
                format!("NOT ({joined})")
            } else {
                format!("({joined})")
            }
        }
    })
}

pub(crate) fn order_sql(rref: &str, terms: &[OrderTerm]) -> String {
    terms
        .iter()
        .map(|t| {
            let mut s = field_sql(rref, &t.field);
            match t.dir {
                Some(Dir::Asc) => s.push_str(" ASC"),
                Some(Dir::Desc) => s.push_str(" DESC"),
                None => {}
            }
            match t.nulls {
                Some(Nulls::First) => s.push_str(" NULLS FIRST"),
                Some(Nulls::Last) => s.push_str(" NULLS LAST"),
                None => {}
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn child_path(path: &[String], e: &Embed) -> Vec<String> {
    let mut p = path.to_vec();
    p.push(e.key().to_string());
    p
}

/// ` LIMIT n OFFSET m`, either or neither.
pub(crate) fn limit_sql(limit: Option<u64>, offset: Option<u64>) -> String {
    let mut s = String::new();
    if let Some(l) = limit {
        s.push_str(&format!(" LIMIT {l}"));
    }
    if let Some(o) = offset.filter(|o| *o > 0) {
        s.push_str(&format!(" OFFSET {o}"));
    }
    s
}

impl<'a> Builder<'a> {
    pub fn new(cache: &'a SchemaCache, schema: &str, q: &'a Query) -> Self {
        Builder { cache, schema: schema.to_string(), q, n: 0 }
    }

    fn alias(&mut self) -> String {
        self.n += 1;
        format!("pgrst_{}", self.n)
    }

    /// The relationship `parent` → `e` (PostgREST's rules, without
    /// many-to-many through a junction table).
    fn relationship(&self, parent: &Rel<'a>, e: &Embed) -> Result<RelShip<'a>, ApiError> {
        let Some(pt) = parent.table else {
            return Err(self.no_rel("this result", &e.name));
        };
        let mut found: Vec<(RelShip<'a>, &crate::schema::ForeignKey, &'static str)> = Vec::new();
        for fk in &self.cache.fks {
            // many-to-one: the parent holds the key.
            if fk.schema == pt.schema && fk.table == pt.name {
                let by_table = fk.ref_table == e.name && fk.ref_schema == pt.schema;
                let by_column = fk.columns.len() == 1 && fk.columns[0] == e.name;
                if by_table || by_column {
                    if let Some(target) = self.cache.table(&fk.ref_schema, &fk.ref_table) {
                        let pairs = fk.ref_columns.iter().cloned().zip(fk.columns.iter().cloned()).collect();
                        found.push((RelShip { target, to_one: true, pairs }, fk, "many-to-one"));
                    }
                }
            }
            // one-to-many (or one-to-one): the child holds the key.
            if fk.ref_schema == pt.schema && fk.ref_table == pt.name && fk.schema == pt.schema && fk.table == e.name {
                if let Some(target) = self.cache.table(&fk.schema, &fk.table) {
                    let pairs = fk.columns.iter().cloned().zip(fk.ref_columns.iter().cloned()).collect();
                    let to_one = self.cache.is_unique(&fk.schema, &fk.table, &fk.columns);
                    found.push((RelShip { target, to_one, pairs }, fk, if to_one { "one-to-one" } else { "one-to-many" }));
                }
            }
        }
        if let Some(h) = &e.hint {
            found.retain(|(_, fk, _)| fk.name == *h || fk.columns.contains(h) || fk.ref_columns.contains(h));
        }
        match found.len() {
            0 => Err(self.no_rel(&pt.name, &e.name)),
            1 => Ok(found.pop().map(|(r, _, _)| r).expect("one")),
            _ => {
                let details: Vec<Value> = found
                    .iter()
                    .map(|(_, fk, card)| {
                        serde_json::json!({
                            "cardinality": card,
                            "embedding": format!("{} with {}", pt.name, e.name),
                            "relationship": format!("{} using {}({}) and {}({})", fk.name, fk.table, fk.columns.join(", "), fk.ref_table, fk.ref_columns.join(", ")),
                        })
                    })
                    .collect();
                let opts: Vec<String> = found.iter().map(|(_, fk, _)| format!("'{}!{}'", e.name, fk.name)).collect();
                Err(ApiError::new(300, "PGRST201", format!("Could not embed because more than one relationship was found for '{}' and '{}'", pt.name, e.name))
                    .details(Value::Array(details))
                    .hint(format!("Try changing '{}' to one of the following: {}. Find the desired relationship in the 'details' key.", e.name, opts.join(", "))))
            }
        }
    }

    fn no_rel(&self, parent: &str, child: &str) -> ApiError {
        ApiError::new(400, "PGRST200", format!("Could not find a relationship between '{parent}' and '{child}' in the schema cache"))
            .details(format!("Searched for a foreign key relationship between '{parent}' and '{child}' in the schema '{}', but no matches were found.", self.schema))
    }

    fn join_sql(rs: &RelShip<'_>, child_ref: &str, parent_ref: &str) -> String {
        rs.pairs.iter().map(|(c, p)| format!("{child_ref}.{} = {parent_ref}.{}", qi(c), qi(p))).collect::<Vec<_>>().join(" AND ")
    }

    /// The embeds named in `items`, by key.
    fn embed_by_key<'b>(items: &'b [SelectItem], key: &str) -> Option<&'b Embed> {
        items.iter().find_map(|i| match i {
            SelectItem::Embed(e) if e.key() == key => Some(e),
            _ => None,
        })
    }

    /// The WHERE conditions for `rel` at `path`: its filters (a filter
    /// `rel=is.null` on an embed is an anti-join), and an EXISTS for each
    /// `!inner` embed.
    pub fn where_conds(&mut self, rel: &Rel<'a>, items: &[SelectItem], path: &[String]) -> Result<Vec<String>, ApiError> {
        let mut out = Vec::new();
        let filters: Vec<Cond> = self.q.filters_at(path).cloned().collect();
        for c in &filters {
            if let Cond::Op { field, negated, op: Op::Is, value: Operand::Is(IsVal::Null) } = c {
                if field.path.is_empty() {
                    if let Some(e) = Self::embed_by_key(items, &field.name) {
                        let ex = self.exists_sql(rel, e, path)?;
                        out.push(if *negated { ex } else { format!("NOT {ex}") });
                        continue;
                    }
                }
            }
            out.push(cond_sql(&rel.rref, c)?);
        }
        for i in items {
            if let SelectItem::Embed(e) = i {
                if e.join == JoinKind::Inner {
                    out.push(self.exists_sql(rel, e, path)?);
                }
            }
        }
        Ok(out)
    }

    /// `EXISTS (SELECT 1 FROM <child> WHERE <join> AND <child's own conds>)`.
    fn exists_sql(&mut self, parent: &Rel<'a>, e: &Embed, path: &[String]) -> Result<String, ApiError> {
        let rs = self.relationship(parent, e)?;
        let a = self.alias();
        let child = Rel { table: Some(rs.target), rref: qi(&a) };
        let mut conds = vec![Self::join_sql(&rs, &child.rref, &parent.rref)];
        conds.extend(self.where_conds(&child, &e.select, &child_path(path, e))?);
        Ok(format!("EXISTS (SELECT 1 FROM {} AS {} WHERE {})", rs.target.qualified(), qi(&a), conds.join(" AND ")))
    }

    /// The select list of `rel` at `path`.
    pub fn select_list(&mut self, rel: &Rel<'a>, items: &[SelectItem], path: &[String]) -> Result<Vec<String>, ApiError> {
        let mut out = Vec::new();
        for i in items {
            match i {
                SelectItem::Star => out.push(format!("{}.*", rel.rref)),
                SelectItem::Col { field, alias, cast } => {
                    let mut e = field_sql(&rel.rref, field);
                    if let Some(t) = cast {
                        e = format!("({e})::{t}");
                    }
                    let label = alias.clone().unwrap_or_else(|| field.label());
                    out.push(format!("{e} AS {}", qi(&label)));
                }
                SelectItem::Embed(e) => {
                    if e.select.is_empty() {
                        continue;
                    }
                    out.push(self.embed_sql(rel, e, path)?);
                }
            }
        }
        // Only filter-only embeds: an empty list (`SELECT FROM …`), so each
        // row is `{}`, as PostgREST answers.
        Ok(out)
    }

    fn embed_sql(&mut self, parent: &Rel<'a>, e: &Embed, path: &[String]) -> Result<String, ApiError> {
        let rs = self.relationship(parent, e)?;
        let a = self.alias();
        let child = Rel { table: Some(rs.target), rref: qi(&a) };
        let cp = child_path(path, e);
        let cols = self.select_list(&child, &e.select, &cp)?;
        let mut conds = vec![Self::join_sql(&rs, &child.rref, &parent.rref)];
        conds.extend(self.where_conds(&child, &e.select, &cp)?);
        let order = self.q.order_at(&cp).map(|o| format!(" ORDER BY {}", order_sql(&child.rref, o))).unwrap_or_default();
        let lim = limit_sql(self.q.limit_at(&cp), self.q.offset_at(&cp));
        let inner = format!("SELECT {} FROM {} AS {} WHERE {}{order}{lim}", cols.join(", "), rs.target.qualified(), qi(&a), conds.join(" AND "));
        let r = qi(&format!("{a}_r"));
        let label = qi(e.key());
        Ok(if rs.to_one {
            format!("(SELECT row_to_json({r}) FROM ({inner}) AS {r}) AS {label}")
        } else {
            format!("COALESCE((SELECT json_agg({r}) FROM ({inner}) AS {r}), '[]'::json) AS {label}")
        })
    }
}

/// The one-row answer shape around a CTE `pgrst_source` of shaped rows.
pub(crate) fn wrap_rows(cte: &str, total: &str) -> String {
    format!(
        "WITH pgrst_source AS ({cte}) SELECT {total} AS total, (SELECT count(*) FROM pgrst_source)::text AS page_total, \
         coalesce((SELECT json_agg(_postgrest_t) FROM pgrst_source AS _postgrest_t), '[]'::json)::text AS body"
    )
}

/// The JSON object a function's named arguments come from.
pub(crate) fn rpc_args_sql(f: &Function, args: &Map<String, Value>) -> Result<(String, String), ApiError> {
    // (the CTE that types them, the call's argument list)
    let mut defs = Vec::new();
    let mut call = Vec::new();
    for a in f.inputs() {
        if !args.contains_key(&a.name) {
            continue;
        }
        // The type is the catalog's own `format_type`, never request text.
        defs.push(format!("{} {}", qi(&a.name), a.ty));
        let v = if a.mode == "v" { "VARIADIC " } else { "" };
        call.push(format!("{v}{} := pgrst_args.{}", qi(&a.name), qi(&a.name)));
    }
    if defs.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let obj = Value::Object(args.clone()).to_string();
    crate::sql::check_value(&obj)?;
    Ok((format!("pgrst_args AS (SELECT * FROM json_to_record({}::json) AS _({})), ", lit(&obj), defs.join(", ")), call.join(", ")))
}
