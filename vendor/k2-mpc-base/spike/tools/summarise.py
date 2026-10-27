import json, sys
kb = lambda b: f"{b/1024:.0f}K"
for n in sys.argv[1:]:
    d = json.load(open(n))
    print("==", d["trace"])
    for e in ["yrs", "loro"]:
        if e not in d: continue
        x = d[e]
        print(f" {e:4} ops={x['ops']} text={kb(x['text_bytes'])} local p50/p95={x['local_edit_us']['p50']:.1f}/{x['local_edit_us']['p95']:.1f}us | remote p50/p95/p99={x['remote_apply_us']['p50']:.1f}/{x['remote_apply_us']['p95']:.1f}/{x['remote_apply_us']['p99']:.1f}us total={x['remote_apply_us']['total_ms']:.0f}ms")
        snap = x.get('state_v1_bytes', x.get('snapshot_bytes'))
        alt = ('v2=' + kb(x['state_v2_bytes'])) if 'state_v2_bytes' in x else ('stateonly=' + kb(x['state_only_bytes']) + ' so_heap=' + kb(x['state_only_loaded_heap_bytes']))
        print(f"      snap={kb(snap)} {alt} coldload={x['cold_load_ms']:.1f}ms loaded_heap={kb(x['loaded_heap_bytes'])} replica={kb(x['replica_heap_bytes'])} session={kb(x['session_estimate_bytes'])} budget={kb(x['budget_bytes'])} replay_heap={kb(x['daemon_heap_after_replay_bytes'])} agent p50/p95={x['agent_edit_us']['p50']:.0f}/{x['agent_edit_us']['p95']:.0f}us")
