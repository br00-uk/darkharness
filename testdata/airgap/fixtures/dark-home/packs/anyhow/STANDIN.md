# Stand-in

This directory stands in for an indexed documentation pack. It holds no
chunk index and no manifest.

Real pack fetch and indexing are task units `G1` to `G5`. `dark doctor`'s
"Pack hashes and staleness" check reports this directory's presence and
states plainly that it cannot verify a hash yet; see
`crates/dark-cli/src/doctor.rs`'s `check_pack_hashes`.
