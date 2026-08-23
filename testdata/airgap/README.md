# Air-gap test fixtures

`cargo xtask airgap` (task unit `J5`) uses the files under `fixtures/` to
stand in for a machine after `dark setup` has finished. It copies this
directory into a scratch directory before every run; nothing here changes
on disk.

## `fixtures/dark-home/`

A `$DARK_HOME` layout with one model directory and two pack directories
already present. `dark doctor`'s model and pack checks (task unit `J3`)
count child directories under `$DARK_HOME/models` and `$DARK_HOME/packs`;
these three directories give a non-zero count without claiming to hold
real weights or a real index.

Each `STANDIN.md` file says so directly. None of these three directories
holds real content:

- a model manifest and real weights need task units `B2` to `B7`;
- a pack manifest and a real chunk index need task units `G1` to `G5`.

When those task units land, replace this fixture with the output of a
real `dark setup` run and delete the `STANDIN.md` files.

## `fixtures/repo/`

A minimal Rust crate. The scripted session's "edit a file and run a test"
step (task unit `J5` step 3) works against this crate, not against the
darkharness workspace itself: editing and testing the harness's own
source from inside the test that exercises the harness would make the
test's target and the test's subject the same tree, and a failure in
either would look like a failure in both.

`cargo xtask airgap` runs `git init` on a copy of this directory at
runtime, because `dark`'s repository-root detection looks for a `.git`
directory (see `crates/dark-cli/src/main.rs`'s `repo_root`); this
directory does not check in a `.git` directory of its own, so that a
nested repository never appears in `git status` for darkharness itself.
