# U0 spike: compute jobs through k2-node updates

The scripts for U0 of `prd-compute-update-continuity-v1` (rev 2, §10). Each answers
one question the code can't, and prints a table to a report file. None of them touch
the real node (`k2-node*`, `k2job*`, `k2-jobs*`, `dev.k2.node*`); every unit, user and
file they make starts with `k2spike` and is removed at the end, also on failure.

| Script | What it proves | Needs | Box |
|---|---|---|---|
| `linux-job-vs-root-service.sh` | A walled job (`systemd-run --pipe --wait`, as k2-node-admin starts jobs) keeps running when the root service that started it is stopped, exits or is `kill -9`ed: with today's default `KillMode=control-group` and with 0.46.4's `KillMode=process` (C5). Whether a killed `systemd-run` helper takes its unit with it (R1). Whether a regular-file spool fd passed as stdout through `--pipe` keeps taking writes after the helper is gone (U2). | root; systemd ≥ 245; `K2_SPIKE_OK=1` | a disposable or test Linux box: z13flow or a throwaway VM, once per systemd version in the fleet (k2n4/5/6 run Debian/Ubuntu: one of each version) |
| `macos-holder-vs-launchd.sh` | A double-forked `setsid` job holder survives `launchctl kickstart -k`, `kill -9` and `bootout`/`bootstrap` of a **system-domain** LaunchDaemon run as a hidden user (R2; ptyd S0 only proved a GUI LaunchAgent). A Seatbelt job writes to an inherited spool fd under a denied `state/`. `(deny signal (target others))` (C7): the job can't signal its holder or other processes but can signal its own children. The holder writes `exit.json` when the job ends. | root; macOS 14+; `K2_SPIKE_OK=1` | an idle Mac mini (k2n2-m1 or k2n3-m1; ask Sew if its CI holds one) |
| `spool-punch.sh` | The tailer can free spool space it already read (`fallocate` PUNCH_HOLE on ext4/xfs, `F_PUNCHHOLE` on APFS) while the job appends `O_APPEND` (Q8, R4). What the tailer sees when the job truncates its stdout or jumps 2 GB ahead (C8). | no root; `cc` | one ext4 Linux node, one xfs box if the fleet has one, one Mac mini; run with `--dir` on the node home's filesystem. Fine as a compute job: `k2 compute run <node> --sha <sha> -- bash scripts/test/compute-continuity-spike/spool-punch.sh --dir /var/tmp` |

Run (root ones at the machine, never on a customer box or this Mac):

```sh
K2_SPIKE_OK=1 sudo -E bash scripts/test/compute-continuity-spike/linux-job-vs-root-service.sh
K2_SPIKE_OK=1 sudo -E bash scripts/test/compute-continuity-spike/macos-holder-vs-launchd.sh
bash scripts/test/compute-continuity-spike/spool-punch.sh --dir /var/tmp
```

Reports: `/var/tmp/k2-spike-linux-job.txt`, `/var/tmp/k2-spike-macos-holder.txt`,
`k2-spike-spool-punch.txt` next to the `--dir` temp folder. Each report ends with how
to read it. A row that says "dies" or "unsupported" is a result, not a script failure;
exit 1 means the spike itself broke.

What the results decide:
- Linux rows 4–6 "keeps running": C5 holds with `KillMode=process` (already in 0.46.4's
  `k2-node-admin.service`). Row 7 "dies": R1, plan the D-Bus `StartTransientUnit` path
  (+2–4 days). Rows 1–3 show what 0.46.3 nodes do today.
- macOS restart rows "holder survives, job survives": §5.2's holder design stands (U2).
  The signal row decides whether C7's Seatbelt rule ships (then also run a real build
  under it).
- `spool-punch`: `punch=ok` with a small `max_alloc` keeps Q8's "punch and drop past
  the cap"; an error means R4 (drop without punching, warn in `k2-node status`).
