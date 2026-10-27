//! k2-node: the compute node runtime (prd-k2-compute-nodes-v1 §8).

fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    {
        imp::main()
    }
    #[cfg(not(unix))]
    {
        eprintln!("k2-node: pending k2-node Windows port: the compute node runtime is Unix-only until then");
        std::process::ExitCode::from(2)
    }
}

// pending k2-node Windows port: the compute node runtime is Unix-only until then
#[cfg(unix)]
#[path = "main_unix.rs"]
mod imp;
