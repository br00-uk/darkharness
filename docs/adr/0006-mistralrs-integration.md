# ADR 0006: integrating mistral.rs 0.8.1 (task units B2 to B7)

**Status:** accepted · **Task units:** `B2`, `B3`, `B4`, `B5`, `B6`, `B7`

## Context

Task units `B2` to `B7` bring the real inference engine into
`crates/dark-engine/`, replacing the placeholder crate. The version and
feature flags were decided in advance (`mistralrs = "0.8.1"`, features
`cuda`, `metal`, `flash-attn`), and this ADR is the record of what
building against that version's real, public API actually required —
where it matched the build specification's assumptions and where it did
not, and the two files outside `crates/dark-engine/` this work needed to
touch.

## Decisions carried out as given

- `mistralrs = "0.8.1"` in `[workspace.dependencies]`, declared plain (no
  `default-features = false`): its default build is the CPU backend,
  matching section 4.5's `dark-cpu` artefact.
- `crates/dark-engine/Cargo.toml` gained `cuda = ["mistralrs/cuda"]`,
  `flash-attn = ["mistralrs/flash-attn"]`, `metal = ["mistralrs/metal"]`.
  These names matched mistralrs 0.8.1's own feature names exactly, confirmed
  by reading `mistralrs`'s published `Cargo.toml` directly, so the forward
  is a one-line pass-through with no renaming.
- The workspace `rust-version` in the root `Cargo.toml` rose from `1.85` to
  `1.88`, matching what mistralrs 0.8.1 itself declares as its floor.

## Decision: close ADR 0005's deferred gap in `crates/dark-cli/Cargo.toml`

ADR 0005 named two changes to `crates/dark-cli/Cargo.toml` that task unit
`J4` could not make because it does not own that file, and said the most
likely owner of the fix was whichever task unit next gave `dark-engine` a
`cuda`/`metal` feature to forward to. That is this task unit. Both changes
landed together, in the same change as the forwarding they depend on:

- `[package.metadata.dist]` / `dist = true`, so `cargo-dist` packages a
  binary from `dark-cli` instead of reading its inherited `publish = false`
  as an opt-out.
- `cuda = ["dark-engine/cuda"]`, `metal = ["dark-engine/metal"]`, and a new
  `flash-attn = ["dark-engine/flash-attn"]` feature, forwarding to the
  features `dark-engine`'s own `Cargo.toml` now declares.

ADR 0005's status is updated to point here. This unblocks the `dark-metal`
and `dark-cuda` jobs in `.github/workflows/release.yml`, which read
`if: false` pending exactly this wiring — that file is not touched here,
since it belongs to task unit `J4`, but nothing else blocks flipping those
two jobs on now.

## Where the real API diverged from what the build specification assumed

The build specification's task units describe behaviour, not a literal
mistralrs API surface — reasonably, since it predates checking that
surface. Building against the real, published 0.8.1 crate (source read
directly from the vendored `mistralrs`, `mistralrs-core`, `mistralrs-mcp`,
and `mistralrs-quant` crates, not assumed from memory or from the online
docs) surfaced five places where the literal shape differs from what a
naive reading of the task units would produce.

### 1. There is no per-request seed

Task unit `B7`, step 3 says "apply the seed." mistral.rs 0.8.1's
`SamplingParams` has no seed field at all: the engine seeds one
process-wide `Isaac64Rng` once, from a fixed constant
(`mistralrs_core::engine::SEED`), at start-up, and every request after
that draws from the same stream. There is nothing in the public API for a
caller to seed per request.

This is not a gap in practice, because `SamplingParams::deterministic()`
forces `top_k = Some(1)` — greedy decoding. With exactly one candidate
token at each step, the sampler never consults the random generator: the
seed value is irrelevant when there is nothing left to randomly choose
between. `crates/dark-engine/src/determinism.rs` forces `top_k = Some(1)`
for a deterministic request and documents this reasoning in full;
`dark_contract::Sampling::seed` is still accepted and passed through
unchanged, for API compatibility and so it is still visible in a
transcript, but it is inert against mistral.rs 0.8.1's engine.

### 2. mistral.rs's `logprob` fields are log base 10, not natural log

`crates/dark-engine/src/embed/rerank.rs` needs to convert a `logprob`
field back to a plain probability to score a document. Reading
`mistralrs_core::sampler`'s construction of `TopLogprob` directly
(`logprob: prob.log(10.0)`) showed this is decimal log, not the natural
log a name like "logprob" suggests everywhere else this term appears
(OpenAI's API, most other inference servers). `affirmative_probability`
converts with `10f32.powf(...)`, not `f32::exp(...)`; the wrong base would
have produced a value that still sorted documents in the right order
(monotonic either way) but was not a real probability, which is exactly
the kind of silent-but-wrong bug that would not show up until compared
against another engine's rerank output.

### 3. A streaming tool call arrives whole, not as growing fragments

Task unit `B4`, step 2 says "accumulate tool-call fragments by index,"
which reads as OpenAI-style streaming, where a tool call's JSON arguments
arrive as a string built up one small piece per chunk. Reading
`mistralrs_core::pipeline::sampling`'s streaming-chunk construction shows
mistral.rs parses a tool call to completion internally before it ever
appears in a `Delta`, and populates `Delta.tool_calls` exactly once, with
the call's full `name` and `arguments` already assembled.

`crates/dark-engine/src/stream/response.rs` maps each `ToolCallResponse`
to one `Chunk::ToolCallDelta` carrying the complete argument text as a
single fragment, rather than trying to invent a fragmentation mistral.rs
does not produce. `crates/dark-engine/src/stream/accumulate.rs`'s
`Accumulator` still reassembles by index exactly as the task unit asks:
it handles a single-fragment call and a genuinely multi-fragment one
identically, so nothing here is tied to today's one-shot delivery if a
future mistral.rs version — or a different engine behind the same
`dyn Engine` trait — sends real fragments instead.

### 4. `Engine::tokenize` is synchronous; mistral.rs's tokenizer access is not

`dark_contract::Engine::tokenize` is `fn tokenize(&self, ...) -> Result<usize>`
— synchronous, because `dark-core` calls it inline while assembling a
turn's prefix (Rule 5: the prefix must not change during a turn), and
should not have to await a channel round-trip just to size a string.
`mistralrs::Model` exposes only `async fn tokenize`, which sends a
`TokenizationRequest` through the same channel a generation request uses
— there is no direct, synchronous handle to a model's tokenizer on
`Model` itself.

`RealEngine` resolves this by holding its own `Arc<tokenizers::Tokenizer>`
per registered model, loaded once, synchronously, from the same
`tokenizer.json` mistral.rs itself reads (`tokenizers` is already a
transitive dependency of `mistralrs`; `crates/dark-engine/Cargo.toml` pins
it to the identical `=0.22.2` mistralrs itself requires, so the build does
not carry two copies of it). `RealEngine::register_model` takes this
handle from the caller alongside the loaded `mistralrs::Model`.

### 5. `dark-airlock`'s `Client` cannot be named by type in `dark-engine`

Rule 13 restricts `reqwest` to `dark-airlock` alone; `cargo xtask
check-deps` fails the build if `dark-engine` names it as a direct
dependency. `dark_airlock::Client::get` returns a `reqwest::Response`,
which `dark-engine` may call methods on (the type flows in through
`dark-airlock`'s own public return type) but may not name — in a struct
field, a generic bound, or an `impl` target — because writing
`reqwest::Response` anywhere requires `reqwest` in `crates/dark-engine/Cargo.toml`,
which Rule 13 forbids.

`crates/dark-engine/src/load/download.rs`'s progress-reporting loop is
therefore written twice: once as `drain_to_file`, generic over a small
local `ByteSource` trait that names no `reqwest` type and is fully tested
against a fake, artificially slow source (`start_paused` virtual time, no
real network and no flaky wall-clock timing); once as
`download_via_airlock`, which repeats the same loop inline against a live
`dark_airlock::Client` response, calling only its inherent methods
(`chunk()`, `content_length()`, `status()`), never spelling the type. The
duplication is the seam Rule 13 draws, made visible rather than hidden
behind a trait that could not actually abstract over both paths.

## What each seam is deferred to real hardware

Every one of these compiles against the real 0.8.1 API (`cargo check -p
dark-engine --all-targets` passes with no `unsafe` and no stub types
standing in for real ones), but none of them can run in this sandbox: it
has no accelerator and no model weights on disk, and downloading a
multi-gigabyte model was judged out of scope for what this session can
verify. Each is named individually, not folded into one blanket "needs a
GPU" note, because each needs a different kind of hardware confirmation:

- `crates/dark-engine/src/resident/mod.rs`'s memory estimator: pinned
  against five published Qwen3 configurations
  (`resident::estimate::tests::five_published_models_match_the_hand_computed_total`),
  each read from its real `config.json` and model card on Hugging Face on
  2026-08-23, not assumed. What is missing is the other half of the build
  specification's "within 10% of measured memory" acceptance criterion: an
  actual load of each of the five, with the process's resident set
  measured before and after.
- `crates/dark-engine/src/load/materialize.rs`: builds and calls
  `UqffTextModelBuilder`, `GgufModelBuilder`, and `TextModelBuilder` — the
  actual weight-loading calls — but never runs one, for want of a model
  file.
- `crates/dark-engine/src/embed/mod.rs`'s `embed_via_model` and
  `rerank_via_model`: call `Model::generate_embeddings` and
  `Model::send_chat_request` for real, but need a loaded embedding and
  worker model to actually run.
- `crates/dark-engine/src/stream/live.rs`: the whole live streaming path,
  including the claim (documented in that module) that dropping
  mistral.rs's response receiver is what causes its scheduler to reclaim a
  cancelled sequence's key-value block. `crates/dark-engine/tests/cancel_leak.rs`
  proves this crate's own lease and permit accounting returns to baseline
  over 1000 cancelled turns; it cannot prove mistral.rs's own allocator
  does the same, which needs a live model and a memory measurement.
- `crates/dark-engine/src/tune/rate.rs`: `measure` is tested against
  `dark-engine-fake` only (a dev-dependency, per Rule 17); a real
  tokens-per-second figure needs a real model generating real tokens.
- `dark models pull` against a real Hugging Face repository: the
  download and manifest pipeline is real and network-capable
  (`load::download_via_airlock`, `load::pull`), but this session did not
  run it against a live multi-gigabyte model end to end — see "Ambiguities
  and gaps, named" below for what a real run against a sharded repository
  still needs.

## `deny.toml` had to widen Rule 13's ban, flagged rather than hidden

`cargo deny check bans` failed after `mistralrs` landed: mistral.rs bundles
`hf-hub`, its own optional client for fetching a model from Hugging Face
directly, and `hf-hub` brings both `reqwest` and `ureq` into the graph
under parents (`hf-hub`, `mistralrs`, `mistralrs-core`, `mistralrs-mcp`,
`openai-harmony`) that `deny.toml`'s existing `wrappers` lists did not
name — because those lists were written before mistral.rs existed in the
workspace at all.

This is not code darkharness wrote or chose to add. Rule 12 already
permits `dark-engine` to depend on `mistralrs`; `hf-hub` is part of what
`mistralrs` *is*. `dark-engine`'s own code never calls that path —
`load::materialize` always hands mistral.rs a local path
`dark-airlock` already downloaded, never a bare repository id, so
mistral.rs's own network client is never exercised in practice — but
`cargo deny`'s ban check inspects the dependency graph's shape, not
runtime behaviour, and cannot see that the code path goes unused.

`deny.toml`'s `wrappers` lists for `reqwest` and `ureq` now include the
five mistral.rs-side crate names, with the reasoning above written
directly into the file next to the change, exactly where the next person
to touch it will read it. This is flagged here and in that comment
because it is a real widening of Rule 13's automated check, not a
cosmetic one: a future change to `dark-engine` that *did* call into
mistral.rs's Hugging Face path would now pass this particular check
silently. Nothing about `dark-engine`'s own source makes that call today,
and `cargo xtask check-deps`'s direct-dependency check is unaffected by
this — it still fails the build the moment `dark-engine`'s own
`Cargo.toml` names `reqwest` directly — but the transitive ban is
weaker than the exact promise Rule 13's prose states, and a reviewer
should know that going in rather than discover it later.

## Ambiguities and gaps, named rather than silently resolved

- **`ToolChoice::Required` has no mistral.rs equivalent.** mistral.rs
  0.8.1's `ToolChoice` is `None`, `Auto`, or `Tool(Tool)` — nothing between
  "the model may pick a tool" and "the model must pick this one."
  `stream::request::to_mistralrs_tool_choice` maps `Required` to `Auto`
  rather than failing the request outright. A stricter behaviour (refuse
  a request that asks for `Required`) was rejected as more surprising to a
  caller than a slightly weaker guarantee.
- **Image, audio, and file message parts are not threaded into a
  request.** `stream::request::build` sends only `Part::Text` content;
  `dark_contract::Part::Image` and `Part::File` are silently dropped
  today. `Caps::vision` exists for a model that can see images, but wiring
  raw image bytes through to `mistralrs::MultimodalMessages` needs its own
  decoding step this task unit did not build. Named here so it reads as a
  gap, not a design choice.
- **`dark models pull` assumes a single-file (or single-shard) layout.**
  `crates/dark-cli/src/models.rs::pull_files` hard-codes the file names a
  UQFF, GGUF, or HF in-situ pull fetches (`{quant}-0.uqff` plus its UQFF
  sidecars, `{quant}.gguf`, or the smallest HF in-situ set). A repository
  sharded across many files (a `model-00001-of-00006.safetensors` pattern)
  needs its real file list read from the Hugging Face API first, which is
  not wired. Today, a sharded repository fails partway through with a
  plain download error naming the missing file, not a clean pre-flight
  check.
- **`dark models pull`'s dark-mode source.** `dark-config` (task unit `J2`)
  owns the harness's persistent dark-mode setting, and is not wired into
  `dark-cli` yet. `models.rs` reads the `DARK_OFFLINE` environment
  variable directly — the same one `dark_airlock::child` already
  propagates to spawned tools — as the nearest available source, rather
  than silently assuming the network is open. Whoever wires `dark-config`
  into `dark-cli` should replace this read with the real policy lookup.
- **`dark tune` prints the `[hardware]` section; it does not write it.**
  `dark-config` owns `$DARK_HOME/config.toml`'s merge and write path.
  Writing this section directly would mean guessing at that format's other
  sections from outside the crate that owns it. `dark tune` prints
  ready-to-paste TOML instead.
- **The key-value cache's bytes-per-element is a fixed constant (2, for
  f16), not read per model.** `resident::estimate::KV_CACHE_BYTES_PER_ELEMENT`
  assumes mistral.rs keeps the cache in half precision regardless of the
  compute dtype the weights load in. This matches mistral.rs's normal
  behaviour but is not read from a loaded model's actual configuration,
  because nothing in `mistralrs::MistralRsConfig` exposes it.
- **Rerank asks for the top 20 log probabilities.** `embed::rerank_via_model`
  requests `top_n_logprobs = 20` alongside the generated token, on the
  reasoning that the affirmative token should very likely appear in the
  top 20 even when it is not the model's first choice. This is a judgement
  call, not a value mistral.rs or the build specification names; a real
  model may need a different figure to reliably surface it.

## Consequences

- `crates/dark-engine/` now has real, direct dependencies beyond
  `dark-contract`: `mistralrs`, `dark-airlock`, `tokio` (`fs`, `io-util`),
  `tokio-util`, `sha2`, `sysinfo`, `toml`, `tracing`, `serde`/`serde_json`,
  `async-trait`, `bytes`, `futures-core`/`futures-util`, `ulid`, and
  `tokenizers` (pinned to mistralrs's own version). `cargo xtask
  check-deps` and `cargo deny check` both pass with this set; see the
  verify output attached to the implementation report for the real run.
- Every method on `RealEngine` (`crates/dark-engine/src/lib.rs`) has a
  clean, tested error path for "no model registered for this role class
  yet" — `E_ENGINE_LOAD` with a remedy, never a panic or an unwrapped
  `None`. The success paths for `stream`, `embed`, `rerank`, and
  `tokenize` need a registered model to exercise, which is the hardware
  gap named above.
