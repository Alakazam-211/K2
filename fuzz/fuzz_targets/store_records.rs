//! The store's own record decoders on arbitrary bytes: each refuses with a typed error or
//! decodes to a record whose encoding is a fixpoint (canonical records: the input itself).
//! Never a panic; allocations are bounded by the run's `-malloc_limit_mb`. No byte reaches
//! the engine (records carry engine bytes; they are never decoded here).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&kind, bytes)) = data.split_first() else {
        return;
    };
    let names = k2_mpc_base::fuzzing::records();
    k2_mpc_base::fuzzing::record(names[kind as usize % names.len()], bytes);
});
