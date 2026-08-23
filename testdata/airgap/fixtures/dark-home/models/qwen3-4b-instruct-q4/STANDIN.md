# Stand-in

This directory stands in for a downloaded and converted model. It holds
no weights.

Real model download and UQFF conversion are task units `B2` to `B7`.
`dark doctor`'s "Model manifest hashes" check reports this directory's
presence and states plainly that it cannot verify a hash yet; see
`crates/dark-cli/src/doctor.rs`'s `check_model_manifests`.
