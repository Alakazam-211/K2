# Fixture: every form that must pass

| Bound | States | Result | Job |
|---|---|---|---|
| 2×2 | **5,977** [run ff000001/c_ok] | complete | ff000001 |
| 2×2 with depth | 5,977 / 5 [run ff000001/c_ok] | complete | 12345678 |
| 4×1 | ≥ 61,233, partial (cap 180 s) [run ff000001/p_cap] | stopped at the cap | ff000001 |
| mutant | trace of 3 states [run ff000001/v_x] | violation | ff000001 |
| queue | 38,383 [run ff000001/p_cap:queued] | a field | ff000001 |
| 3×3 | ~10^12 | 4.5·10^10 [estimate], 1,500,000 [estimate] | |
| ratio | 69.1 (10,790,212 / 156,237) [derived] | | |
| old | 1,044,182 [unverified: log unavailable] | | |
| time | 17,116 s, 2,400 MiB | small numbers: 37, 123 | |

Prose is not table text: 44,768,326,790 needs no citation here, but a citation in prose is
still checked: 5,977 [run ff000001/c_ok].

A field name with a digit: 12 [run ff000001/c_ok:concurrent_restore_o4a].
