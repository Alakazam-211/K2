//! The mapper's reading, without Postgres: a table of contents, grants,
//! COPY rows, the reference scan and the migration map, from text shaped as
//! `pg_restore` 17 prints it for a synthetic Supabase-style dump. The real
//! restore is `real_pg.rs`.

use std::collections::BTreeSet;

use super::copy::{decode_field, parse_copy, parse_text_array};
use super::toc::{classify, parse_line, Extract};
use super::*;

/// A table of contents as `pg_restore -l` printed it for the synthetic
/// dump the real-Postgres test makes (trimmed), plus Supabase's usual extras.
const TOC: &str = ";
; Archive created at 2026-11-22 06:08:16 MST
;     Format: CUSTOM
;
; Selected TOC Entries:
;
7; 2615 16389 SCHEMA - auth postgres
8; 2615 16390 SCHEMA - extensions postgres
12; 2615 16395 SCHEMA - app_private postgres
3358; 0 0 ACL - SCHEMA public pg_database_owner
3370; 0 0 ACL - SCHEMA auth supabase_admin
11; 2615 16393 SCHEMA - realtime postgres
9; 2615 16391 SCHEMA - storage postgres
2; 3079 16394 EXTENSION - pgcrypto
3359; 0 0 COMMENT - EXTENSION pgcrypto
3; 3079 16431 EXTENSION - uuid-ossp
3360; 0 0 COMMENT - EXTENSION \"uuid-ossp\"
4; 3079 16432 EXTENSION - pg_graphql
5; 3079 16433 EXTENSION - pg_stat_statements
6; 3079 16434 EXTENSION - postgis
273; 1255 16469 FUNCTION auth uid() postgres
274; 1255 16473 FUNCTION public f(integer, text) postgres
3361; 0 0 ACL public FUNCTION f(a integer, b text) postgres
224; 1259 16442 TABLE auth users postgres
226; 1259 16456 TABLE public profiles postgres
3362; 0 0 ACL public TABLE profiles postgres
227; 1259 16457 TABLE storage objects supabase_storage_admin
3349; 0 16442 TABLE DATA auth users postgres
3351; 0 16456 TABLE DATA public profiles postgres
3350; 0 16449 TABLE DATA supabase_migrations schema_migrations postgres
3352; 0 16457 TABLE DATA storage objects supabase_storage_admin
3353; 0 16458 TABLE DATA storage buckets supabase_storage_admin
3200; 2620 16476 TRIGGER auth users on_auth_user_created postgres
3201; 2620 16477 TRIGGER storage objects update_objects_updated_at supabase_storage_admin
3199; 2606 16464 FK CONSTRAINT public profiles profiles_id_fkey postgres
3347; 3256 16470 POLICY public profiles Own rows, with space postgres
3348; 3256 16471 POLICY storage objects Avatars are readable postgres
3346; 0 16456 ROW SECURITY public profiles postgres
3354; 6104 16474 PUBLICATION - supabase_realtime postgres
3355; 3466 16480 EVENT TRIGGER - issue_graphql_placeholder supabase_admin
2107; 826 16471 DEFAULT ACL public DEFAULT PRIVILEGES FOR TABLES postgres
";

/// The two pending triggers' definitions, as `pg_restore -s` prints them.
const TRIGGERS: &str = "--
-- Name: users on_auth_user_created; Type: TRIGGER; Schema: auth; Owner: -
--

CREATE TRIGGER on_auth_user_created AFTER INSERT ON auth.users FOR EACH ROW EXECUTE FUNCTION public.handle_new_user();

--
-- Name: objects update_objects_updated_at; Type: TRIGGER; Schema: storage; Owner: -
--

CREATE TRIGGER update_objects_updated_at BEFORE UPDATE ON storage.objects FOR EACH ROW EXECUTE FUNCTION storage.update_updated_at_column();
";

fn trusted() -> BTreeSet<String> {
    ["pgcrypto", "uuid-ossp", "pg_trgm", "citext"].iter().map(|s| s.to_string()).collect()
}

#[test]
fn toc_lines_parse_with_multi_word_kinds_and_spaces_in_tags() {
    let e = parse_line("3347; 3256 16470 POLICY public profiles Own rows, with space postgres").unwrap();
    assert_eq!((e.desc, e.ns.as_str(), e.first(), e.owner()), ("POLICY", "public", "profiles", "postgres"));
    assert_eq!(e.tag(), "profiles Own rows, with space");
    let e = parse_line("3349; 0 16442 TABLE DATA auth users postgres").unwrap();
    assert_eq!((e.desc, e.ns.as_str(), e.first()), ("TABLE DATA", "auth", "users"));
    let e = parse_line("2107; 826 16471 DEFAULT ACL public DEFAULT PRIVILEGES FOR TABLES postgres").unwrap();
    assert_eq!(e.desc, "DEFAULT ACL");
    let e = parse_line("2; 3079 16394 EXTENSION - pgcrypto ").unwrap();
    assert_eq!((e.desc, e.first(), e.owner()), ("EXTENSION", "pgcrypto", ""));
    assert!(parse_line("; Selected TOC Entries:").is_none());
    assert!(parse_line("not; a line").is_none());
}

#[test]
fn the_table_of_contents_is_sorted_into_keep_grants_extracts_and_drops() {
    let mut p = classify(TOC, &trusted());
    assert_eq!(p.pending_triggers.len(), 2, "triggers on auth.users and storage.objects wait for their definitions");
    toc::decide_triggers(&mut p, TRIGGERS);
    assert!(p.pending_triggers.is_empty());
    let kept = p.keep.join("\n");
    for k in ["SCHEMA - app_private", "EXTENSION - pgcrypto", "COMMENT - EXTENSION \"uuid-ossp\"", "FUNCTION public f", "TABLE public profiles", "TABLE DATA public profiles", "POLICY public profiles", "ROW SECURITY public profiles", "FK CONSTRAINT"] {
        assert!(kept.contains(k), "kept: {k}\n{kept}");
    }
    assert!(kept.contains("TRIGGER auth users on_auth_user_created"), "the App's sign-up trigger stays on K2's auth.users");
    assert!(p.kept_on_supabase_tables.iter().any(|k| k.contains("public.handle_new_user")), "{:?}", p.kept_on_supabase_tables);
    assert!(kept.contains("POLICY storage objects Avatars are readable"), "the App's Storage policy stays");
    for d in ["SCHEMA - auth", "SCHEMA - storage", "FUNCTION auth uid", "TABLE auth users", "update_objects_updated_at", "pg_graphql", "PUBLICATION", "EVENT TRIGGER", "TABLE storage objects"] {
        assert!(!kept.contains(d), "dropped: {d}");
    }
    assert_eq!(p.acl.len(), 4, "public schema, f(), profiles, default privileges: {:?}", p.acl);
    assert!(!p.acl.iter().any(|a| a.contains("SCHEMA auth")), "grants on a dropped schema go with it");
    assert_eq!(p.extract.keys().copied().collect::<Vec<_>>(), vec![Extract::AuthUsers, Extract::Migrations, Extract::StorageBuckets, Extract::StorageObjects]);
    assert_eq!(p.carried_extensions.iter().cloned().collect::<Vec<_>>(), vec!["pgcrypto", "uuid-ossp"]);
    assert!(p.dropped_extensions.contains("pg_graphql") && p.dropped_extensions.contains("pg_stat_statements"));
    assert!(p.dropped_schemas.contains_key("auth") && p.dropped_schemas.contains_key("realtime") && p.dropped_schemas.contains_key("extensions"));
    assert_eq!(p.unresolved.len(), 1, "postgis isn't on the box: {:?}", p.unresolved);
    assert!(p.unresolved.iter().next().unwrap().object.contains("postgis"));
    assert!(p.notes.iter().any(|n| n.contains("supabase_realtime")) && p.notes.iter().any(|n| n.contains("pg_stat_statements")));
    assert_eq!((p.tables, p.policies, p.functions), (1, 2, 1));
}

/// The grants as `pg_restore -f - --no-owner` printed them.
const ACL_SCRIPT: &str = "--
-- Name: SCHEMA public; Type: ACL; Schema: -; Owner: -
--

GRANT USAGE ON SCHEMA public TO anon;
GRANT USAGE ON SCHEMA public TO postgres;
REVOKE ALL ON FUNCTION public.f(a integer, b text) FROM PUBLIC;
GRANT ALL ON FUNCTION public.f(a integer, b text) TO authenticated;
GRANT ALL ON TABLE public.profiles TO anon;
GRANT ALL ON TABLE public.profiles TO service_role;
GRANT INSERT,REFERENCES,DELETE,TRIGGER,TRUNCATE,MAINTAIN,UPDATE ON TABLE public.profiles TO supabase_admin;
GRANT SELECT ON TABLE public.profiles TO supabase_admin WITH GRANT OPTION;
GRANT SELECT ON TABLE public.profiles TO \"App Reporter\";
SET SESSION AUTHORIZATION supabase_admin;
GRANT SELECT ON TABLE public.profiles TO anon;
RESET SESSION AUTHORIZATION;
ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public GRANT ALL ON TABLES TO anon;
ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public GRANT ALL ON TABLES TO dashboard_user;
ALTER DEFAULT PRIVILEGES FOR ROLE supabase_admin IN SCHEMA public GRANT ALL ON TABLES TO service_role;
ALTER DEFAULT PRIVILEGES FOR ROLE \"App Owner\" IN SCHEMA public GRANT ALL ON TABLES TO anon;
";

#[test]
fn grants_map_to_the_standard_roles_and_postgres_defaults_become_the_migrators() {
    let out = acl::rewrite(ACL_SCRIPT, "ws_x_migrator").unwrap();
    assert_eq!(
        out.statements,
        vec![
            "GRANT USAGE ON SCHEMA public TO anon;",
            "REVOKE ALL ON FUNCTION public.f(a integer, b text) FROM PUBLIC;",
            "GRANT ALL ON FUNCTION public.f(a integer, b text) TO authenticated;",
            "GRANT ALL ON TABLE public.profiles TO anon;",
            "GRANT ALL ON TABLE public.profiles TO service_role;",
            "GRANT SELECT ON TABLE public.profiles TO anon;",
            "ALTER DEFAULT PRIVILEGES FOR ROLE \"ws_x_migrator\" IN SCHEMA public GRANT ALL ON TABLES TO anon;",
        ]
    );
    assert_eq!(out.kept, 7);
    assert_eq!(out.dropped_internal, 5, "postgres, supabase_admin ×2, dashboard_user, supabase_admin's defaults");
    assert_eq!(out.dropped_roles.iter().cloned().collect::<Vec<_>>(), vec!["App Owner", "App Reporter"]);
    let err = acl::rewrite("GRANT SELECT ON TABLE public.x\n  TO anon;\n", "m").unwrap_err();
    assert!(err.contains("spans lines"), "{err}");
    assert!(acl::rewrite("COMMENT ON TABLE x IS 'y';\n", "m").is_err(), "never guesses at a statement it doesn't read");
}

#[test]
fn copy_blocks_and_text_arrays_decode() {
    let script = "SET row_security = off;\n\nCOPY supabase_migrations.schema_migrations (version, statements, name) FROM stdin;\n\
20240202120000\t\\N\t\\N\n\
20240101120000\t{\"create table a (x int)\",\"with \\\\\"quote\\\\\" and \\\\\\\\back, comma\"}\tinit\n\\.\n\n";
    let t = parse_copy(script, "supabase_migrations.schema_migrations").unwrap().expect("a block");
    assert_eq!(t.columns, vec!["version", "statements", "name"]);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.get(&t.rows[0], "name"), None);
    let arr = parse_text_array(t.get(&t.rows[1], "statements").unwrap()).unwrap();
    assert_eq!(arr, vec![Some("create table a (x int)".to_string()), Some("with \"quote\" and \\back, comma".to_string())]);
    assert_eq!(parse_text_array("{a,NULL,\"b c\"}").unwrap(), vec![Some("a".into()), None, Some("b c".into())]);
    assert_eq!(parse_text_array("{}").unwrap(), Vec::<Option<String>>::new());
    assert!(parse_text_array("{{1}}").is_err());
    assert_eq!(decode_field("a\\tb\\nc\\\\d\\101\\x42").unwrap().as_deref(), Some("a\tb\nc\\dAB"));
    assert_eq!(decode_field("\\N").unwrap(), None);
    assert!(parse_copy("COPY auth.users (id) FROM stdin;\nx\n", "auth.users").is_err(), "a block that never ends");
    assert!(parse_copy("INSERT INTO auth.users VALUES (1);\n", "auth.users").unwrap_err().contains("--inserts"));
    assert_eq!(parse_copy("nothing here\n", "auth.users").unwrap(), None);
    let quoted = parse_copy("COPY public.t (id, \"Odd, Name\") FROM stdin;\n1\tx\n\\.\n", "public.t").unwrap().unwrap();
    assert_eq!(quoted.columns, vec!["id", "Odd, Name"]);
}

#[test]
fn the_scan_lists_what_k2_does_not_carry_and_never_what_it_does() {
    let script = "--\n-- Name: notify(); Type: FUNCTION; Schema: public; Owner: -\n--\n\
CREATE FUNCTION public.notify() RETURNS void LANGUAGE plpgsql AS $$ BEGIN PERFORM net.http_post('https://x'); PERFORM cron.schedule('a', '* * * * *', 'select 1'); END $$;\n\
--\n-- Name: secrets_v; Type: VIEW; Schema: public; Owner: -\n--\n\
CREATE VIEW public.secrets_v AS SELECT decrypted_secret FROM vault.decrypted_secrets;\n\
--\n-- Name: me(); Type: FUNCTION; Schema: public; Owner: -\n--\n\
CREATE FUNCTION public.me() RETURNS uuid LANGUAGE sql AS $$ SELECT \"auth\".\"uid\"() FROM auth.users u JOIN auth.identities i ON i.user_id = u.id $$;\n\
CREATE TABLE public.t (id uuid DEFAULT extensions.uuid_generate_v4(), my_net int, public_net int);\n\
SELECT x.my_net, storage.foldername(name), storage.search('a') FROM public.net_things x;\n\
--\n-- Name: t p; Type: POLICY; Schema: public; Owner: -\n--\n\
CREATE POLICY p ON public.t FOR SELECT TO authenticated, supabase_auth_admin USING ((auth.uid() = id));\n\
CREATE POLICY q ON public.t TO anon USING (true);\n";
    let found: Vec<(String, String)> = scan::scan(script).into_iter().map(|u| (u.object, u.reference)).collect();
    let refs: Vec<&str> = found.iter().map(|(_, r)| r.as_str()).collect();
    assert!(refs.contains(&"net.http_post") && refs.contains(&"cron.schedule") && refs.contains(&"vault.decrypted_secrets"), "{found:?}");
    assert!(refs.contains(&"auth.identities") && refs.contains(&"storage.search"), "{found:?}");
    assert!(refs.iter().any(|r| r.starts_with("role supabase_auth_admin")), "{found:?}");
    assert!(found.iter().any(|(o, r)| o == "FUNCTION public.notify()" && r == "net.http_post"), "named by object: {found:?}");
    assert_eq!(found.len(), 6, "auth.uid, auth.users, extensions.*, storage.foldername, net_things and anon are fine: {found:?}");
}

#[test]
fn migrations_are_numbered_in_version_order_with_their_statements() {
    let t = parse_copy(
        "COPY supabase_migrations.schema_migrations (version, statements, name) FROM stdin;\n\
20240202120000\t{\"alter table a add y int;\"}\tAdd Y!\n\
20240101120000\t{\"create table a (x int)\",\"create index on a (x)\"}\tinit\n\
20240303120000\t\\N\t\\N\n\\.\n",
        "supabase_migrations.schema_migrations",
    )
    .unwrap()
    .unwrap();
    let m = migrations::plan(&t).unwrap();
    let names: Vec<&str> = m.iter().map(|x| x.k2_version.as_str()).collect();
    assert_eq!(names, vec!["0001_init", "0002_add_y", "0003_migration"]);
    assert!(m[0].sql.contains("\ncreate table a (x int);\n\ncreate index on a (x);\n"), "{}", m[0].sql);
    assert!(m[1].sql.contains("alter table a add y int;\n") && !m[1].sql.contains(";;"));
    assert!(m[2].sql.contains("kept no statements"));
    let map = migrations::map_json(&m);
    assert_eq!(map["migrations"][1]["supabase"], "20240202120000");
    assert_eq!(map["migrations"][1]["renamed"], "0002_add_y");
}

#[test]
fn rows_are_written_with_quoted_values_and_force_is_found() {
    let t = parse_copy("COPY auth.users (id, email, encrypted_password, is_anonymous) FROM stdin;\nu1\to'neil@example.com\t$2a$x\t\\N\n\\.\n", "auth.users").unwrap().unwrap();
    let sql = insert_sql("auth.users", &t, &t.rows, standin::AUTH_USER_COLUMNS, &["is_anonymous"]);
    assert_eq!(sql, "INSERT INTO auth.users (\"id\", \"email\", \"is_anonymous\") VALUES\n('u1', 'o''neil@example.com', false);\n");
    assert!(!sql.contains("$2a$"), "never the password");
    assert_eq!(forced_in("ALTER TABLE ONLY public.a FORCE ROW LEVEL SECURITY;\nALTER TABLE public.b ENABLE ROW LEVEL SECURITY;\n"), vec!["public.a"]);
    assert!(standin::roles_ok("anon|false|false|false\nauthenticated|false|false|false\nservice_role|false|false|true\n").is_ok());
    assert!(standin::roles_ok("anon|true|false|false\nauthenticated|false|false|false\nservice_role|false|false|true\n").unwrap_err().starts_with("roles_conflict"));
    assert!(standin::roles_sql().contains("CREATE ROLE service_role NOLOGIN NOINHERIT BYPASSRLS;"));
}

#[test]
fn the_route_needs_a_file_and_a_database() {
    let r = handle(b"{\"project\":\"x\",\"file\":\"d.dump\"}");
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert!(r.body.contains("--db"), "{}", r.body);
    let r = handle(b"not json");
    assert_eq!(r.status, "400 Bad Request");
}
