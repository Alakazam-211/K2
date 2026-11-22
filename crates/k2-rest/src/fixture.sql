-- The grammar tests' fixture (and the oracle rig's): a small app shaped like
-- the reference app's data (orgs, people, projects, tasks), with every kind
-- of relationship the door resolves and a few functions. Deterministic data.
CREATE TABLE public.orgs (id int PRIMARY KEY, name text NOT NULL);
CREATE TABLE public.profiles (
  id uuid PRIMARY KEY,
  org_id int REFERENCES public.orgs(id),
  name text,
  email text UNIQUE,
  meta jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT '2026-01-01T00:00:00Z'
);
CREATE TABLE public.projects (
  id serial PRIMARY KEY,
  org_id int NOT NULL REFERENCES public.orgs(id),
  owner_id uuid REFERENCES public.profiles(id),
  reviewer_id uuid REFERENCES public.profiles(id),
  title text NOT NULL,
  status text NOT NULL DEFAULT 'open',
  tags text[] NOT NULL DEFAULT '{}',
  data jsonb,
  due date,
  budget numeric(12,2)
);
CREATE TABLE public.tasks (
  id serial PRIMARY KEY,
  project_id int NOT NULL REFERENCES public.projects(id) ON DELETE CASCADE,
  title text NOT NULL,
  done boolean NOT NULL DEFAULT false,
  pos int
);
CREATE TABLE public.project_settings (
  project_id int PRIMARY KEY REFERENCES public.projects(id) ON DELETE CASCADE,
  theme text
);
CREATE TABLE public.devices (
  id uuid PRIMARY KEY,
  owner_id uuid NOT NULL REFERENCES public.profiles(id),
  label text NOT NULL,
  UNIQUE (owner_id, label)
);
CREATE VIEW public.open_projects_v AS SELECT id, org_id, title FROM public.projects WHERE status = 'open';

INSERT INTO public.orgs VALUES (1, 'Acme'), (2, 'Globex'), (3, 'Empty Co');
INSERT INTO public.profiles (id, org_id, name, email, meta) VALUES
  ('11111111-1111-4111-8111-111111111111', 1, 'Ann', 'ann@example.com', '{"role": "admin", "langs": ["en", "fr"]}'),
  ('22222222-2222-4222-8222-222222222222', 1, 'Bo', 'bo@example.com', '{"role": "member"}'),
  ('33333333-3333-4333-8333-333333333333', 2, 'Cy', NULL, '{}'),
  ('44444444-4444-4444-8444-444444444444', NULL, NULL, 'dee@example.com', '{"role": "guest"}');
INSERT INTO public.projects (id, org_id, owner_id, reviewer_id, title, status, tags, data, due, budget) VALUES
  (1, 1, '11111111-1111-4111-8111-111111111111', '22222222-2222-4222-8222-222222222222', 'Alpha', 'open', '{red,blue}', '{"kind": "web", "score": 5}', '2026-03-01', 1000.50),
  (2, 1, '22222222-2222-4222-8222-222222222222', NULL, 'Beta, the second', 'closed', '{blue}', '{"kind": "app", "score": 2}', NULL, NULL),
  (3, 2, '33333333-3333-4333-8333-333333333333', '11111111-1111-4111-8111-111111111111', 'Gamma', 'open', '{}', NULL, '2026-05-15', 20),
  (4, 1, NULL, NULL, 'delta lowercase', 'open', '{green}', '{"kind": "web"}', NULL, 0),
  (5, 2, '11111111-1111-4111-8111-111111111111', NULL, 'Epsilon %_ odd', 'archived', '{red}', '{"kind": "ops", "score": 9}', '2026-01-10', 99.99);
SELECT setval('public.projects_id_seq', 5);
INSERT INTO public.tasks (id, project_id, title, done, pos) VALUES
  (1, 1, 'a1', true, 2), (2, 1, 'a2', false, 1), (3, 1, 'a3', false, NULL),
  (4, 2, 'b1', true, 1), (5, 3, 'c1', false, 1), (6, 5, 'e1', false, 3);
SELECT setval('public.tasks_id_seq', 6);
INSERT INTO public.project_settings VALUES (1, 'dark'), (3, 'light');
INSERT INTO public.devices VALUES ('aaaaaaaa-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', 'phone');

CREATE FUNCTION public.add(a int, b int DEFAULT 10) RETURNS int LANGUAGE sql IMMUTABLE AS $$ SELECT a + b $$;
CREATE FUNCTION public.org_projects(p_org int) RETURNS SETOF public.projects LANGUAGE sql STABLE AS $$ SELECT * FROM public.projects WHERE org_id = p_org ORDER BY id $$;
CREATE FUNCTION public.first_project() RETURNS public.projects LANGUAGE sql STABLE AS $$ SELECT * FROM public.projects ORDER BY id LIMIT 1 $$;
CREATE FUNCTION public.project_titles() RETURNS TABLE (pid int, title text) LANGUAGE sql STABLE AS $$ SELECT id, title FROM public.projects ORDER BY id $$;
CREATE FUNCTION public.tag_list() RETURNS SETOF text LANGUAGE sql STABLE AS $$ SELECT DISTINCT unnest(tags) FROM public.projects ORDER BY 1 $$;
CREATE FUNCTION public.close_project(p_id int) RETURNS void LANGUAGE sql AS $$ UPDATE public.projects SET status = 'closed' WHERE id = p_id $$;
CREATE FUNCTION public.echo_json(json) RETURNS json LANGUAGE sql IMMUTABLE AS $$ SELECT $1 $$;
CREATE FUNCTION public.fail_teapot() RETURNS int LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'no tea' USING ERRCODE = 'PT418', DETAIL = 'empty pot'; END $$;

GRANT USAGE ON SCHEMA public TO anon, authenticated, service_role;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO anon;
GRANT ALL ON ALL TABLES IN SCHEMA public TO authenticated, service_role;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO authenticated, service_role;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO anon, authenticated, service_role;
