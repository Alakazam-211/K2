# dfn3/

K2's noise removal (Sew's `Standard` mode) runs the DeepFilterNet3 model
(low-latency variant), whose weights ship with the app in this folder:

- `dfn3_ll.nnef.tar`: the weights (MIT OR Apache-2.0; converted by Alakazam
  Labs, see `NOTICE`)
- `NOTICE`: their notice
- `LICENSE-MIT` and `LICENSE-APACHE`: the authors' licence texts

They are not in git. `scripts/fetch-dfn3.sh` downloads them from the pinned
Sew release, checks both sha256s and stages them here; every app build runs
it. Without them, noise removal falls back to RNNoise.
