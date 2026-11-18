# Test data: the DWG fixture

| File | Size (bytes) | SHA-256 |
|---|---|---|
| `draw_sampler.dwg` | 11852 | `9672014d7f5ff45c8f768ab8bb81bbc9a001068be1d27adff61f36e47c134461` |

- **What it is:** the drawing of `k2_mpc_dxf::testgen::draw_sampler` (its 2018 DXF), as one DWG
  file of version `AC1032` (2018), each object under the handle the DXF gives it. Its app-info
  names our own test tooling; it holds no trusted-file marker, no security flags, no encrypted
  section and no password, and its date fields are zero.
- **Provenance:** made by our own test tooling (not published; the commit it was made at is in
  the repo's `PROVENANCE.md`) from our own drawing (`k2_mpc_dxf::testgen::draw_sampler`, R2018,
  from the CAD pack v0.3.0), not by any vendor's software.
- **Made once, with kit v0.22.0**, against the CAD pack's DWG reader in the release that adds this file. Nothing in this
  repo writes DWG, at test time or anywhere else; a new file would be a new fixture, cleared
  again before it is committed.
- The empty 3DSOLID's tail after the empty-data flags is not verified against a real file; readers should treat this object as opaque.
- **Read by** `crates/cad-measure/tests/dwg_fixture.rs`: the DWG reader's drawing compared with
  the DXF the test generator writes, every row and the header equal (the known items: a
  viewport's on flag, and every row's `read-only` edit).
