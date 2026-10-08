//! k2-node: the compute node runtime (prd-k2-compute-nodes-v1 §8).

fn main() {
    println!("k2-node {} (protocol {})", env!("CARGO_PKG_VERSION"), k2_node_proto::PROTOCOL);
}
