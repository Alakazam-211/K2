-- 0170: the 0.46.0 wall knobs on `apps` (prd-k2-app-runtime-v1 §4.3, §4.4,
-- §6.2; runtime lane D). New nullable/defaulted columns only. 0166/0167
-- are Sew's, 0168 is the App grace release and 0169 App egress. The full `app_walls`
-- migration (path/egress/ports/capabilities config, 0.46.1) folds these in.
--
-- io_max_json     per-App disk ceilings (cgroup v2 io.max via systemd
--                 IO*Max): {"rbps":N,"wbps":N,"riops":N,"wiops":N}, each
--                 a number or null (= no ceiling). NULL row = K2's default
--                 ceiling (stage W §5.1: wbps 40 MiB/s, 2000 write IOPS).
-- devices_json    the `process` exception's device grants (enum tokens,
--                 `dri-render128` …), each one an owner widening.
-- exception_json  who approved the `process` exception, when and how (the
--                 owner confirm method); NULL = never approved.
ALTER TABLE apps ADD COLUMN io_max_json TEXT;
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN devices_json TEXT NOT NULL DEFAULT '[]';
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN exception_json TEXT;
