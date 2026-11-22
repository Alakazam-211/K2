//! The schema cache (PRD §7.1): tables, views, columns, keys, foreign keys
//! and functions of an App's exposed schemas, read once with
//! [`introspect_sql`] (one row, one JSON value) and kept by the daemon.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::sql::{lit, qi};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Column {
    pub name: String,
    /// `format_type` of the column.
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub nullable: bool,
    /// `pg_get_expr` of its default, if any.
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Table {
    pub schema: String,
    pub name: String,
    /// `r` table, `v` view, `m` materialized view, `f` foreign, `p` partitioned.
    pub kind: String,
    pub columns: Vec<Column>,
    #[serde(default)]
    pub pk: Vec<String>,
}

impl Table {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }
    pub fn qualified(&self) -> String {
        format!("{}.{}", qi(&self.schema), qi(&self.name))
    }
}

/// One foreign key: `schema.table(columns)` → `ref_schema.ref_table(ref_columns)`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Unique {
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Arg {
    /// Empty for an unnamed argument.
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    /// `i` in, `o` out, `b` inout, `v` variadic, `t` table column.
    pub mode: String,
    #[serde(default)]
    pub has_default: bool,
}

impl Arg {
    pub fn is_input(&self) -> bool {
        matches!(self.mode.as_str(), "i" | "b" | "v")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Function {
    pub schema: String,
    pub name: String,
    pub args: Vec<Arg>,
    pub returns_set: bool,
    /// `format_type` of the return type.
    pub ret_type: String,
    /// `typtype` of the return type: `b` base, `c` composite, `d` domain,
    /// `e` enum, `p` pseudo (void, record), `r` range, `m` multirange.
    pub ret_kind: String,
    /// The relation whose row type it returns (a table's), if any.
    #[serde(default)]
    pub ret_rel: Option<(String, String)>,
    /// `i`, `s` or `v`.
    pub volatility: String,
}

impl Function {
    /// Returns rows with columns (a row type, a table, `RETURNS TABLE`, OUT
    /// arguments).
    pub fn returns_rows(&self) -> bool {
        self.ret_kind == "c" || (self.ret_type == "record" && self.args.iter().any(|a| matches!(a.mode.as_str(), "o" | "b" | "t")))
    }
    pub fn returns_void(&self) -> bool {
        self.ret_type == "void"
    }
    pub fn inputs(&self) -> impl Iterator<Item = &Arg> {
        self.args.iter().filter(|a| a.is_input())
    }
}

/// The cache for one database.
#[derive(Debug, Clone, Default)]
pub struct SchemaCache {
    pub tables: HashMap<(String, String), Table>,
    pub fks: Vec<ForeignKey>,
    pub uniques: Vec<Unique>,
    pub functions: Vec<Function>,
}

#[derive(Deserialize)]
struct Raw {
    tables: Vec<Table>,
    fks: Vec<ForeignKey>,
    uniques: Vec<Unique>,
    functions: Vec<Function>,
}

impl SchemaCache {
    /// Read the answer of [`introspect_sql`].
    pub fn from_json(v: &Value) -> Result<SchemaCache, String> {
        let raw: Raw = serde_json::from_value(v.clone()).map_err(|e| format!("schema cache: {e}"))?;
        let tables = raw.tables.into_iter().map(|t| ((t.schema.clone(), t.name.clone()), t)).collect();
        Ok(SchemaCache { tables, fks: raw.fks, uniques: raw.uniques, functions: raw.functions })
    }

    pub fn table(&self, schema: &str, name: &str) -> Option<&Table> {
        self.tables.get(&(schema.to_string(), name.to_string()))
    }

    /// Are exactly `cols` (in any order) a unique key or the primary key of
    /// `schema.table`?
    pub fn is_unique(&self, schema: &str, table: &str, cols: &[String]) -> bool {
        let same = |a: &[String]| a.len() == cols.len() && a.iter().all(|c| cols.contains(c));
        self.uniques.iter().any(|u| u.schema == schema && u.table == table && same(&u.columns))
            || self.table(schema, table).is_some_and(|t| !t.pk.is_empty() && same(&t.pk))
    }

    pub fn functions_named<'a>(&'a self, schema: &'a str, name: &'a str) -> impl Iterator<Item = &'a Function> + 'a {
        self.functions.iter().filter(move |f| f.schema == schema && f.name == name)
    }
}

/// One statement whose one row and column is the cache for `schemas`, as
/// JSON text. Reads only the catalogs (any role may).
pub fn introspect_sql(schemas: &[String]) -> String {
    let arr = format!("ARRAY[{}]::text[]", schemas.iter().map(|s| lit(s)).collect::<Vec<_>>().join(", "));
    format!(
        r#"SELECT json_build_object(
  'tables', (SELECT coalesce(json_agg(t), '[]'::json) FROM (
     SELECT n.nspname AS schema, c.relname AS name, c.relkind::text AS kind,
       (SELECT coalesce(json_agg(json_build_object('name', a.attname, 'type', format_type(a.atttypid, a.atttypmod),
                 'nullable', NOT a.attnotnull, 'default', pg_get_expr(d.adbin, d.adrelid)) ORDER BY a.attnum), '[]'::json)
          FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
          WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped) AS columns,
       (SELECT coalesce(json_agg(a.attname ORDER BY array_position(i.indkey::int2[], a.attnum)), '[]'::json)
          FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
          WHERE i.indrelid = c.oid AND i.indisprimary) AS pk
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE n.nspname = ANY({arr}) AND c.relkind IN ('r', 'v', 'm', 'f', 'p')) t),
  'fks', (SELECT coalesce(json_agg(f), '[]'::json) FROM (
     SELECT con.conname AS name, ns.nspname AS schema, cl.relname AS "table",
       (SELECT json_agg(a.attname ORDER BY k.ord) FROM unnest(con.conkey) WITH ORDINALITY k(n, ord)
          JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n) AS columns,
       fns.nspname AS ref_schema, fcl.relname AS ref_table,
       (SELECT json_agg(a.attname ORDER BY k.ord) FROM unnest(con.confkey) WITH ORDINALITY k(n, ord)
          JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.n) AS ref_columns
     FROM pg_constraint con
       JOIN pg_class cl ON cl.oid = con.conrelid JOIN pg_namespace ns ON ns.oid = cl.relnamespace
       JOIN pg_class fcl ON fcl.oid = con.confrelid JOIN pg_namespace fns ON fns.oid = fcl.relnamespace
     WHERE con.contype = 'f' AND (ns.nspname = ANY({arr}) OR fns.nspname = ANY({arr}))) f),
  'uniques', (SELECT coalesce(json_agg(u), '[]'::json) FROM (
     SELECT ns.nspname AS schema, cl.relname AS "table",
       (SELECT json_agg(a.attname ORDER BY k.ord) FROM unnest(con.conkey) WITH ORDINALITY k(n, ord)
          JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n) AS columns
     FROM pg_constraint con JOIN pg_class cl ON cl.oid = con.conrelid JOIN pg_namespace ns ON ns.oid = cl.relnamespace
     WHERE con.contype IN ('u', 'p') AND ns.nspname = ANY({arr})
     UNION ALL
     SELECT ns.nspname, cl.relname,
       (SELECT json_agg(a.attname ORDER BY k.ord) FROM unnest(i.indkey::int2[]) WITH ORDINALITY k(n, ord)
          JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.n)
     FROM pg_index i JOIN pg_class cl ON cl.oid = i.indrelid JOIN pg_namespace ns ON ns.oid = cl.relnamespace
     WHERE i.indisunique AND i.indpred IS NULL AND i.indexprs IS NULL AND ns.nspname = ANY({arr})) u),
  'functions', (SELECT coalesce(json_agg(p), '[]'::json) FROM (
     SELECT n.nspname AS schema, p.proname AS name,
       (SELECT coalesce(json_agg(json_build_object(
                 'name', coalesce(p.proargnames[i], ''),
                 'type', format_type(coalesce(p.proallargtypes[i], p.proargtypes[i - 1]), NULL),
                 'mode', coalesce(p.proargmodes[i]::text, 'i'),
                 'has_default', (coalesce(p.proargmodes[i]::text, 'i') IN ('i', 'b', 'v')
                                 AND (SELECT count(*) FROM generate_series(i + 1, coalesce(array_length(p.proallargtypes, 1), p.pronargs)) j
                                      WHERE coalesce(p.proargmodes[j]::text, 'i') IN ('i', 'b', 'v')) < p.pronargdefaults)
               ) ORDER BY i), '[]'::json)
          FROM generate_series(1, coalesce(array_length(p.proallargtypes, 1), p.pronargs)) i) AS args,
       p.proretset AS returns_set,
       format_type(p.prorettype, NULL) AS ret_type,
       t.typtype::text AS ret_kind,
       (SELECT json_build_array(rn.nspname, rc.relname) FROM pg_class rc JOIN pg_namespace rn ON rn.oid = rc.relnamespace
          WHERE rc.oid = t.typrelid AND rc.relkind IN ('r', 'v', 'm', 'f', 'p')) AS ret_rel,
       p.provolatile::text AS volatility
     FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace JOIN pg_type t ON t.oid = p.prorettype
     WHERE n.nspname = ANY({arr}) AND p.prokind = 'f') p)
)::text"#
    )
}
