# darkharness — Build Specification

**Version:** 1.0 · **Status:** Approved for build · **Language:** Simplified Technical English (ASD-STE100 writing rules)

---

## 1 How to use this document

### 1.1 Purpose

This document tells a team of agents how to build darkharness. It is complete. An agent does not need any other document to do the work. External links in this document are for background only. Do not read them to complete a task.

### 1.2 Structure

The work is divided into **task units**. Each task unit has an identifier, for example `E4`. Each task unit contains these fields:

| Field | Meaning |
| --- | --- |
| **Goal** | What the task unit must achieve. |
| **Owns** | The files that this task unit creates or changes. No other task unit changes these files. |
| **Needs** | The task units that must be complete first. |
| **Do** | The instructions. Obey them in the given sequence. |
| **Do not** | Prohibited actions. These prevent known failures. |
| **Verify** | Commands that show the work is correct. Run them. |
| **Done when** | The conditions that make the task unit complete. |

### 1.3 Rules for the orchestrator

1. Dispatch task unit `Z1` first. Wait for it to complete. Do not dispatch other task units before `Z1` is complete.
2. Dispatch task unit `B1` second. Many task units need `B1`.
3. After `Z1` and `B1` are complete, dispatch task units in parallel. Obey the **Needs** field.
4. Give each subagent one task unit. Do not give a subagent two task units at the same time.
5. Give each subagent the full text of its task unit. Also give it Section 2, Section 3, and Section 4.
6. Run an independent verifier subagent on each completed task unit. The verifier runs the **Verify** commands. The verifier does not read the implementer's notes.
7. If a verifier fails a task unit, dispatch the task unit again. Give the new subagent the verifier's report.

### 1.4 Rules for each subagent

1. Change only the files in your **Owns** field.
2. If you must change a file that you do not own, stop. Write an ADR in `docs/adr/`. Report to the orchestrator.
3. Write the tests before you write the code.
4. Run the **Verify** commands before you report completion.
5. Do not add dependencies that Section 4.4 prohibits.
6. Do not change `crates/dark-contract/` unless you own task unit `Z1`.

### 1.5 Language rules for all written output

Write all documentation, all code comments, and all error messages with these rules:

1. Use the active voice.
2. Write one instruction in one sentence.
3. Keep instruction sentences to 20 words or fewer.
4. Keep descriptive sentences to 25 words or fewer.
5. Use the same word for the same thing every time. Section 2 gives the approved terms.
6. Use simple verb tenses.
7. Use articles. Do not remove `the` or `a` to make a sentence shorter.
8. Do not use noun clusters of more than three words.
9. Put the topic in the first sentence of each paragraph.
10. Use a vertical list for complex information.
11. Put a warning before the step that needs it.

---

## 2 Approved terms

Use these terms. Do not use a different word for the same thing.

| Term | Meaning |
| --- | --- |
| **harness** | The darkharness application. |
| **engine** | The component that runs a model. It contains mistral.rs. |
| **model** | A set of weights that the engine loads. |
| **resident model** | A model that is in memory now. |
| **resident set** | All resident models. |
| **role class** | A purpose for a model. The role classes are architect, worker, scout, embed, and rerank. |
| **micro-role** | A configuration of one resident model. A micro-role changes sampling and thinking. A micro-role does not change the model. |
| **turn** | One exchange. A turn starts with an input. A turn ends when the model stops and calls no more tools. |
| **round-trip** | One tool call and its result inside a turn. |
| **session** | A sequence of turns with one message history. |
| **sub-session** | A session that another session starts. A sub-session has its own message history. |
| **prefix** | The part of the context that does not change during a turn. |
| **tail** | The part of the context that grows during a turn. |
| **granted context** | The context length that the resident set manager gives to a turn. |
| **map** | A wayfinder map. It holds tickets, fog, and scope exclusions. |
| **ticket** | One decision to resolve. A ticket is a child of a map. |
| **frontier** | The tickets that are open, unblocked, and unclaimed. |
| **fog** | A question that you cannot yet state precisely. |
| **digest** | The compact text form of a map. |
| **pack** | A local corpus of documentation for one library at one version. |
| **chunk** | One retrievable part of a pack. |
| **seam** | A place in the code where a change has a bounded effect. |
| **blast radius** | The set of symbols that a change can affect. |
| **dark mode** | The state where the harness blocks all network egress. |
| **airlock** | The component that blocks network egress. |

---

## 3 Product definition

### 3.1 What the harness is

darkharness is a local coding harness. It is one Rust binary. It contains its own inference engine, its own task tracker, its own documentation index, and a full-screen terminal application.

### 3.2 The primary requirement

After `dark setup` completes, the user disconnects the network. The user continues to work. Every design decision in this document supports this requirement.

### 3.3 The six components

| Component | Crate | Function |
| --- | --- | --- |
| Engine | `dark-engine` | Loads models. Runs inference. Manages the resident set. |
| Core | `dark-core` | Runs the turn loop. Assembles context. Carries events. |
| Cartograph | `dark-cartograph` | Stores maps, tickets, edges, and fog. |
| Lexicon | `dark-lexicon` | Indexes and retrieves documentation packs. |
| Compass | `dark-explore` | Analyses the repository. Computes seams. |
| Horizon | `dark-tui` | Shows the terminal application. |

### 3.4 The models

The harness supports Qwen models only. It supports all sizes. The engine loads Hugging Face weights, GGUF files, and UQFF files.

### 3.5 The commands

```
dark                          Start the terminal application.
dark run "<prompt>" [--dark]  Run one turn. Show no interface.
dark setup                    Configure the harness. Download models.
dark tune                     Measure the hardware. Write the profile.
dark doctor [--offline]       Check the installation.
dark models {list,pull,quantize,rm,verify}
dark pack {add,list,refresh,rm,export,import,reindex}
dark map {list,show,export,rebuild,health}
dark explore [path] [--json] [--refresh]
dark seams [path] [--top N]
dark blast <symbol>
dark agents explain           Show the resolved instruction chain.
dark session {list,replay,resume}
dark config {get,set,explain}
dark stats
dark update
```

In-session commands:

```
/plan "<idea>"       Chart a map.
/plan work [ticket]  Work the map.
/explore [path]      Analyse the repository.
/seams [path]        Show the seam report.
/docs <lib> <topic>  Search the Lexicon.
/map                 Open the fog map.
/godark              Enter dark mode.
/golight             Leave dark mode.
/residency           Show the resident set.
/model <class> <id>  Override a role class.
/think on|off|auto   Set the thinking mode.
/compact             Compact the context now.
/clear /help /quit
```

---

## 4 Constraints

These constraints come from hardware limits and from the standards that the harness follows. Obey them. Do not design around them.

### 4.1 Memory is the dominant limit

The harness runs every model on one machine. Each model uses memory. A model load takes seconds.

Calculate the memory that a model needs:

```
weights   = parameters × bits_per_weight / 8
kv_cache  = 2 × layers × kv_heads × head_dim × context_length × bytes_per_element
total     = weights + kv_cache + 10% headroom
```

A 30B model at 4 bits needs approximately 17 GB for weights. A 32k KV cache adds 1 GB to 4 GB.

**Rule 1.** On a machine with less than 24 GB, the architect, worker, and scout role classes share one resident model. This is the default configuration. It is not a failure state.

**Rule 2.** The embedding model is pinned. The resident set manager never evicts it.

**Rule 3.** The resident set manager never evicts a model during a turn.

**Rule 4.** The resident set manager estimates memory before a load. It refuses a load that does not fit. It reports the shortfall.

### 4.2 The context prefix must not change during a turn

The engine caches the key-value tensors for the context prefix. A stable prefix makes a round-trip fast. A changed prefix causes a full prefill. A full prefill takes 15 to 30 seconds on a 32B model.

**Rule 5.** Assemble the prefix at the start of a turn. Do not change it during the turn.

**Rule 6.** Do not put a clock in the prefix. Put the date in the prefix. Do not put the time in the prefix.

**Rule 7.** Compact the context only at a turn boundary.

**Rule 8.** Append new content to the tail. Do not insert content into the prefix.

### 4.3 Central processor inference is slow

A turn with thinking generates 1000 to 3000 tokens. A central processor generates a few tokens each second on a 14B model. This makes a turn take minutes.

**Rule 9.** `dark doctor` measures the generation rate. It reports the rate and a hardware class.

**Rule 10.** On a central-processor-only machine, the default profile uses a 4B model. Thinking is off. The round-trip limit is 12.

**Rule 11.** The README states the hardware floor. The coding loop needs a graphics processor or Apple Silicon.

### 4.4 Dependency rules

These rules control build time and make the airlock auditable.

**Rule 12.** Only `dark-engine` depends on `mistralrs`.

**Rule 13.** Only `dark-airlock` constructs an HTTP client. `cargo-deny` prohibits `reqwest`, `hyper`, and `ureq` in every other crate.

**Rule 14.** `dark-tui` depends on `dark-contract` only. It receives events. It sends intents.

**Rule 15.** `dark-contract` depends on `serde`, `thiserror`, `ulid`, `bytes`, and `tokio` only.

**Rule 16.** `dark-explore`, `dark-lexicon`, and `dark-cartograph` depend on `dark-contract` and their own storage crates only.

**Rule 17.** Every crate except `dark-engine` builds against `dark-engine-fake` during development.

### 4.5 Build targets

The harness produces three artefacts. One binary does not run everywhere.

| Artefact | Features | Platforms |
| --- | --- | --- |
| `dark-cpu` | default | All. This artefact is portable. |
| `dark-metal` | `metal` | macOS arm64. |
| `dark-cuda` | `cuda,flash-attn` | Linux and Windows x64 with NVIDIA. |

**Rule 18.** `dark doctor` detects the accelerator. It warns if the binary is `dark-cpu` and the machine has a usable graphics processor.

### 4.6 Human-in-the-loop rules

The wayfinder method defines two ticket kinds. A human-in-the-loop ticket needs a live exchange with a person. The agent must not answer for the person.

**Rule 19.** `ticket_resolve` on a human-in-the-loop ticket fails with `E_HITL_REQUIRES_HUMAN`. It succeeds only when the session holds a human-present token. The terminal application grants this token after the person confirms in a modal.

**Rule 20.** A session resolves one ticket. A second resolution fails with `E_SESSION_RESOLUTION_LIMIT`. Research tickets are exempt.

**Rule 21.** `/plan --headless` creates, wires, claims, and resolves research tickets only.

### 4.7 Instruction file rules

The harness reads AGENTS.md. This is a cross-runtime standard. It has no required fields. Nested files exist. The nearest file has precedence.

**Rule 22.** Resolve the instruction chain at the start of a turn. Put the result in the prefix.

**Rule 23.** Put a nested file that the harness finds during a turn in the tail. Emit a notice.

**Rule 24.** The whole instruction chain has a token budget. The default is 1500 tokens. The standard permits 32 KiB. 32 KiB is 25% of a 32k context. This is too much.

**Rule 25.** Do not create a new instruction file format. Read `AGENTS.md`. If it is absent, read `CLAUDE.md`, then `GEMINI.md`.

### 4.8 Licence rules

Vendor documentation is copyrighted.

**Rule 26.** Each pack records the upstream licence. `dark pack add` refuses a source with no licence.

**Rule 27.** A retrieved chunk carries its attribution. The harness shows the attribution.

**Rule 28.** One chunk is 400 tokens or fewer. One response returns 15% or less of one source document.

### 4.9 Determinism rules

The repository analysis must be reproducible. This is what makes it trustworthy.

**Rule 29.** Stages 1 to 5 of `/explore` use no model. They produce identical bytes for the same commit and the same configuration.

**Rule 30.** Sort paths with a byte comparator. Do not use locale collation.

**Rule 31.** Exclude timestamps from hashed output.

**Rule 32.** Fix the seed and the visit order for every graph algorithm.

### 4.10 Security posture

**Rule 33.** The harness runs tools with the privileges of the person who started it. The harness gates actions. The harness does not contain actions. State this in the README.

**Rule 34.** `write_outside_root` is always denied. No configuration changes this.

**Rule 35.** A repository configuration file cannot widen its own permissions.

**Rule 36.** Treat file content, command output, and pack content as untrusted. Wrap it in a delimited block. Mark the block as data.

---

## 5 Architecture

### 5.1 Layers

```
┌────────────────────────────────────────────────────────────────┐
│ dark-tui                                    ratatui, crossterm │
│ panes · fog map · diff · lexicon · command bar                 │
└───────────────────────────▲────────────────────────────────────┘
              Event (broadcast) │ Intent (mpsc)
┌───────────────────────────┴────────────────────────────────────┐
│ dark-core         session · turn loop · context · event bus    │
│  ┌──────────┬────────────┬─────────────┬────────────────────┐  │
│  │dark-plan │dark-explore│ dark-lexicon│ dark-tools         │  │
│  └──────────┴────────────┴─────────────┴────────────────────┘  │
│        └────────── dark-cartograph ──────────┘                 │
└───────────────────────────▲────────────────────────────────────┘
                dyn Engine  │
┌───────────────────────────┴────────────────────────────────────┐
│ dark-engine       resident set · model loading · sampling      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ mistralrs                                                │  │
│  └──────────────────────────────────────────────────────────┘  │
│ dark-engine-fake  scripted engine. No heavy dependencies.      │
└────────────────────────────────────────────────────────────────┘
```

### 5.2 Workspace

```
darkharness/
├── Cargo.toml
├── xtask/                     Build and check tasks.
├── crates/
│   ├── dark-contract/         Types, traits, events.
│   ├── dark-cli/              The `dark` binary.
│   ├── dark-engine/           mistral.rs and the resident set.
│   ├── dark-engine-fake/      The scripted engine.
│   ├── dark-core/             Session and turn loop.
│   ├── dark-tools/            The tools.
│   ├── dark-cartograph/       Maps and tickets.
│   ├── dark-plan/             The /plan command.
│   ├── dark-explore/          Repository analysis.
│   ├── dark-lexicon/          Documentation retrieval.
│   ├── dark-tui/              The terminal application.
│   ├── dark-qwen/             Prompts, profiles, tool-call parsing.
│   ├── dark-agentsmd/         AGENTS.md resolution.
│   ├── dark-config/           Configuration.
│   └── dark-airlock/          The only HTTP client.
├── testdata/
└── docs/adr/
```

### 5.3 Storage

```
$DARK_HOME (default ~/.darkharness)
├── config.toml
├── models/<repo>/<revision>/{weights, manifest.toml}
├── packs/<name>@<version>/
├── maps/<map-id>/{journal.jsonl, assets/}
├── sessions/<ulid>/transcript.jsonl
└── telemetry.jsonl

<repo>/.dark/
├── config.toml                Project settings. Commit this file.
├── cartograph.db              Derived. Do not commit this file.
└── explore/<sha>.{json,lock}  Derived. Do not commit these files.
```

---

## 6 Task units

### Dispatch order

```
Z1 ──► B1 ──► everything else
                │
   ┌────┬───────┼────────┬────────┬────────┬────────┐
   ▼    ▼       ▼        ▼        ▼        ▼        ▼
  A*   B2-B7   D*       F*       G*       H*       J*
   │    │       │        │        │        │
   └─┬──┴───┬───┘        │        │        │
     ▼      ▼            │        │        │
    C*     I*  ──────────┘        │        │
     └──────┬──────────────────────┘        │
            ▼                               │
           K*  ────────────────────────────►│
            ▼
           E*
```

---

## Z — Contract freeze

### Z1 · Create the workspace and the contract

**Goal.** Produce a workspace that compiles. Produce the shared types and traits. Block all other work until this is complete.

**Owns.** `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `xtask/`, `crates/dark-contract/`, `crates/dark-cli/src/main.rs`, `.github/workflows/ci.yml`.

**Needs.** Nothing.

**Do.**

1. Create a Cargo workspace. Use Rust edition 2024. Pin the toolchain in `rust-toolchain.toml`.
2. Add these profile settings to the workspace `Cargo.toml`:
   ```toml
   [profile.dev]
   opt-level = 1
   [profile.dev.package."*"]
   opt-level = 3
   ```
3. Create every crate directory from Section 5.2. Each crate compiles and is empty.
4. Write the message types in `dark-contract`:
   ```rust
   pub enum Role { System, User, Assistant, Tool }

   pub enum Part {
       Text(String),
       Image { data: Bytes, mime: String },
       File  { path: PathBuf, mime: String },
   }

   pub struct ToolCall { pub id: String, pub name: String,
                         pub args: serde_json::Value }

   pub struct Message {
       pub role: Role,
       pub parts: Vec<Part>,
       pub tool_calls: Vec<ToolCall>,
       pub tool_call_id: Option<String>,
       /// Thinking text. dark-qwen lifts this out of <think> blocks.
       /// The harness never sends this field to a model.
       pub reasoning: Option<String>,
       /// A pinned message goes in the prefix. See Rule 5.
       pub pinned: bool,
   }
   ```
5. Write the engine types:
   ```rust
   pub enum RoleClass { Architect, Worker, Scout, Embed, Rerank }
   pub enum ThinkMode { Auto, On, Off }
   pub enum EmbedPurpose { Query, Document }

   pub struct Sampling {
       pub temperature: Option<f32>,
       pub top_p: Option<f32>,
       pub top_k: Option<usize>,
       pub min_p: Option<f32>,
       pub presence_penalty: Option<f32>,
       pub repetition_penalty: Option<f32>,
       pub seed: Option<u64>,
   }

   pub struct Request {
       pub class: RoleClass,
       pub messages: Vec<Message>,
       pub tools: Vec<ToolSchema>,
       pub tool_choice: ToolChoice,
       pub sampling: Sampling,
       pub think: ThinkMode,
       pub max_tokens: usize,
       pub stop: Vec<String>,
       pub grammar: Option<Grammar>,
       pub deterministic: bool,
   }

   pub enum Chunk {
       Text(String),
       Reasoning(String),
       ToolCallDelta { index: usize, id: Option<String>,
                       name: Option<String>, args_fragment: String },
       Usage(Usage),
       ModelLoading { model: String, progress: f32 },
       Done(FinishReason),
   }

   pub struct Caps {
       pub model_id: String,
       pub max_context: usize,
       /// The context that the resident set manager grants now.
       /// A caller budgets against this field. See Rule 4.
       pub granted_context: usize,
       pub native_tools: bool,
       pub thinking: bool,
       pub grammar: bool,
       pub vision: bool,
       pub logprobs: bool,
       pub params_b: f32,
       pub quant: String,
       pub device: Device,
       pub measured_tok_s: Option<f32>,
   }
   ```
6. Write the `Engine` trait:
   ```rust
   #[async_trait::async_trait]
   pub trait Engine: Send + Sync + 'static {
       async fn caps(&self, class: RoleClass) -> Result<Caps>;

       /// The token cancels the request. A dropped stream also cancels.
       /// The engine releases the key-value cache block on cancellation.
       async fn stream(&self, req: Request, cancel: CancellationToken)
           -> Result<BoxStream<'static, Result<Chunk>>>;

       async fn embed(&self, texts: Vec<String>, purpose: EmbedPurpose)
           -> Result<Vec<Vec<f32>>>;

       async fn rerank(&self, query: &str, docs: Vec<String>)
           -> Result<Vec<Scored>>;

       fn tokenize(&self, class: RoleClass, text: &str) -> Result<usize>;

       fn residency(&self) -> ResidencySnapshot;
   }
   ```
7. Write the tool types:
   ```rust
   pub struct ToolSchema {
       pub name: String,
       pub description: String,
       pub parameters: serde_json::Value,
       pub tier: u8,        // 1 essential, 2 standard, 3 advanced
       pub mutating: bool,
   }

   #[async_trait::async_trait]
   pub trait Tool: Send + Sync {
       fn schema(&self) -> ToolSchema;
       async fn invoke(&self, args: serde_json::Value, ctx: &ToolCtx)
           -> Result<ToolResult>;
   }

   pub struct ToolCtx {
       pub root: PathBuf,
       pub events: EventTx,
       pub cancel: CancellationToken,
       pub dark: bool,
       pub human_present: bool,
   }
   ```
8. Write the event and intent types:
   ```rust
   pub enum Event {
       SessionStart { id: String, root: PathBuf },
       TurnStart    { turn: String, class: RoleClass, model: String },
       TokenDelta   { turn: String, text: String },   // lossy
       ReasonDelta  { turn: String, text: String },   // lossy
       ModelLoading { model: String, progress: f32 },
       ToolCall     { turn: String, call: ToolCall },
       ToolProgress { turn: String, call_id: String, line: String },
       ToolResult   { turn: String, call_id: String,
                      result: ToolResultSummary },
       TurnEnd      { turn: String, usage: Usage, wall_ms: u64 },
       Budget       { used: usize, granted: usize },
       Residency    (ResidencySnapshot),
       DarkChanged  { dark: bool },
       MapChanged   { map_id: String },
       ExploreDone  { tree_sha: String, path: PathBuf },
       IndexProgress{ pack: String, done: usize, total: usize },
       ConfirmReq   { id: String, prompt: ConfirmPrompt },
       Error        { code: ErrCode, msg: String, remedy: Option<String> },
       Notice       (String),
   }

   pub enum Intent {
       Submit(String), Cancel, Confirm { id: String, allow: Allow },
       Command(String), GoDark(bool), Quit,
   }
   ```
9. Create two broadcast channels. One channel carries `TokenDelta` and `ReasonDelta`. The second channel carries every other event. A slow subscriber loses only the first channel.
10. Write the error taxonomy. Use these prefixes: `E_ENGINE_`, `E_TOOL_`, `E_POLICY_`, `E_MAP_`, `E_PACK_`, `E_EXPLORE_`, `E_HITL_`, `E_SESSION_`. Each error carries a code, a message, and an optional remedy.
11. Write `xtask check-deps`. It parses `cargo metadata`. It asserts Rules 12 to 17.
12. Write the CI workflow. It runs `cargo clippy -- -D warnings`, `cargo nextest run`, and `xtask check-deps`.

**Do not.**

- Do not add `mistralrs`, `ratatui`, `reqwest`, or `rusqlite` to `dark-contract`.
- Do not name the binary `dh`. That name shadows the Debian helper tool.

**Verify.**

```
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo xtask check-deps
cargo doc --no-deps -p dark-contract
```

**Done when.** The workspace compiles on Linux, macOS, and Windows. `check-deps` passes. Every public item in `dark-contract` has a documentation comment.

---

## B — Engine

### B1 · Build the fake engine

**Goal.** Produce a scripted engine. Seven other task units need it. Build it before the real engine.

**Owns.** `crates/dark-engine-fake/`.

**Needs.** `Z1`.

**Do.**

1. Implement `Engine` for `FakeEngine`.
2. Load scripted responses from a TOML file. One file holds many turns.
3. Stream text one token at a time. Make the delay configurable.
4. Support injected tool calls, injected errors, and cancellation.
5. Produce fake embeddings from a hash of the input. Make the cosine similarity controllable. Retrieval tests need this.
6. Return a configurable `ResidencySnapshot`.
7. Return a configurable `Caps`. Tests use this to simulate a 4B model and a 32B model.

**Do not.**

- Do not add any dependency that takes more than two seconds to compile.

**Verify.**

```
cargo build -p dark-engine-fake --timings
cargo nextest run -p dark-engine-fake
```

**Done when.** A clean build of `dark-engine-fake` takes less than 10 seconds. The engine conformance suite passes.

---

### B2 · Load models

**Goal.** Load a Qwen model from three formats. Report progress.

**Owns.** `crates/dark-engine/src/load/`.

**Needs.** `Z1`.

**Do.**

1. Add `mistralrs` to `dark-engine`. No other crate adds it.
2. Support three formats. Prefer them in this order:

   | Format | Load speed | Use for |
   | --- | --- | --- |
   | UQFF | Fastest | Every configured profile. |
   | GGUF | Fast | Files that the user already holds. |
   | Hugging Face with in-situ quantisation | Slowest | First use only. |

3. Make UQFF the default. The resident set manager swaps models. A slow load makes a swap unusable.
4. Download weights through `dark-airlock`. Dark mode blocks a download.
5. Write a manifest for each model. Record the repository, the revision, the quantisation, the SHA-256 hash, and the measured memory use.
6. Emit `Chunk::ModelLoading` at 2 Hz or faster during a load.

**Verify.**

```
cargo nextest run -p dark-engine load::
cargo run -p dark-cli -- models pull Qwen/Qwen3-4B --quant uqff-q4k
```

**Done when.** A model loads from each of the three formats. Progress events arrive at 2 Hz or faster.

---

### B3 · Build the resident set manager

**Goal.** Control which models are in memory. Prevent memory exhaustion.

**Owns.** `crates/dark-engine/src/resident/`.

**Needs.** `Z1`.

**Do.**

1. Define the state:
   ```rust
   pub struct ResidentSet {
       budget_bytes: u64,
       slots: HashMap<ModelKey, Slot>,   // Loaded | Loading | Evicted
       pinned: HashSet<ModelKey>,
       lru: VecDeque<ModelKey>,
       turn_leases: HashMap<TurnId, ModelKey>,
   }
   ```
2. Estimate memory before a load. Use the formula in Section 4.1. Read the layer count, the key-value head count, and the head dimension from the model configuration.
3. Refuse a load that does not fit. Return `E_ENGINE_WONT_FIT`. State the shortfall in bytes.
4. Never evict a pinned model. Pin the embedding model by default.
5. Never evict a model that holds a turn lease.
6. Evict by least recent use. Consider only unpinned models without a lease.
7. Compute `Caps::granted_context` from the memory that remains after the weights.
8. Apply this degradation sequence when a request does not fit. Report each step:
   1. Reduce the requested context.
   2. Use a smaller quantisation if one is on disk.
   3. Alias the role class to a smaller class.
   4. Refuse. State the remedy.
9. Emit `Event::Residency` on every change.

**Do not.**

- Do not discover a memory limit by allocation failure. Estimate first.
- Do not evict a model during a turn.

**Verify.**

```
cargo nextest run -p dark-engine resident::
```

**Done when.** The estimator is within 10% of measured memory for five models. The eviction tests pass. `E_ENGINE_WONT_FIT` reports a correct shortfall.

---

### B4 · Stream, sample, and cancel

**Goal.** Convert engine output to `Chunk`. Cancel cleanly.

**Owns.** `crates/dark-engine/src/stream/`.

**Needs.** `Z1`, `B2`.

**Do.**

1. Map the engine response stream to `Chunk`.
2. Accumulate tool-call fragments by index.
3. Feed the raw text to the tool-call scraper when the engine does not parse tool calls. See `I3`.
4. Release the sequence and its key-value block on cancellation.
5. Limit concurrent sequences. Read the limit from the resident set headroom. Parallel sub-sessions consume memory.

**Verify.**

```
cargo nextest run -p dark-engine stream::
cargo nextest run -p dark-engine --test cancel_leak
```

**Done when.** 1000 cancelled turns return memory to the baseline.

---

### B5 · Embed and rerank

**Goal.** Produce vectors. Score documents.

**Owns.** `crates/dark-engine/src/embed/`.

**Needs.** `Z1`, `B3`.

**Do.**

1. Use the pinned embedding model for `embed`.
2. Batch the input. A pack index contains thousands of chunks. Per-call overhead dominates.
3. Apply a different prefix for `EmbedPurpose::Query` and `EmbedPurpose::Document`. An asymmetric model needs this. A wrong prefix halves retrieval quality.
4. Implement `rerank` as single-token scoring. Use a fixed prompt. Set `max_tokens` to 1. Read the log probability of the affirmative token.
5. Gate `rerank` on `Caps::logprobs`. Return `E_ENGINE_UNSUPPORTED` when the field is false.

**Do not.**

- Do not implement rerank with a second embedding pass. That is not reranking.

**Verify.**

```
cargo nextest run -p dark-engine embed::
```

**Done when.** Embeddings batch correctly. Prefixes differ by purpose. Rerank returns an error when log probabilities are absent.

---

### B6 · Add hardware measurement

**Goal.** Measure the machine. Write the profile.

**Owns.** `crates/dark-engine/src/tune/`.

**Needs.** `Z1`, `B2`, `B3`.

**Do.**

1. Detect the device. Report central processor, CUDA, or Metal.
2. Read the total memory and the available memory.
3. Run a short generation. Measure tokens each second.
4. Classify the machine. Use the generation rate and the memory.
5. Write the `[hardware]` section of the configuration:
   ```toml
   [hardware]
   device = "metal"
   memory_total_gb = 36.0
   memory_budget_gb = 26.0
   measured_tok_s = { "qwen3-14b-q4" = 41.2 }
   ```
6. Recommend a profile. Show the model, the quantisation, the context, the expected rate, and whether the role classes share one model.

**Verify.**

```
cargo run -p dark-cli -- tune
cargo nextest run -p dark-engine tune::
```

**Done when.** `dark tune` writes a valid profile on a test machine.

---

### B7 · Add deterministic generation

**Goal.** Make generation reproducible for tests.

**Owns.** `crates/dark-engine/src/determinism.rs`.

**Needs.** `Z1`, `B4`.

**Do.**

1. When `Request::deterministic` is true, set the batch size to 1.
2. Disable attention paths that reorder reductions.
3. Apply the seed.
4. Document the limit. Reproducibility holds for one build on one device. It does not hold across devices or engine versions.

**Verify.**

```
cargo nextest run -p dark-engine determinism::
```

**Done when.** Ten runs with the same seed produce identical output.

---

## A — Core runtime

### A1 · Build the session and the transcript

**Goal.** Record every event. Rebuild a session from the record.

**Owns.** `crates/dark-core/src/session/`.

**Needs.** `Z1`, `B1`.

**Do.**

1. Write one JSON object for each event to `$DARK_HOME/sessions/<ulid>/transcript.jsonl`.
2. Rebuild `Session::messages` by replay.
3. Flush to disk on `TurnEnd` and on `MapChanged`. Do not flush on a token delta.
4. Hold this state:
   ```rust
   pub struct Session {
       pub id: Ulid,
       pub root: PathBuf,
       pub messages: Vec<Message>,
       pub budget: Budget,
       pub dark: bool,
       pub human_present: bool,
       pub resolved_this_session: u32,
       prefix_hash: u64,
   }
   ```

**Verify.**

```
cargo nextest run -p dark-core session::
```

**Done when.** Replay of a transcript reproduces the message list exactly.

---

### A2 · Build the turn loop

**Goal.** Run one exchange. Call tools. Stop correctly.

**Owns.** `crates/dark-core/src/turn/`.

**Needs.** `Z1`, `B1`, `A1`.

**Do.**

1. Resolve the model for the role class. A load may block. Emit `ModelLoading`.
2. Assemble the context. See `A3`.
3. Call `Engine::stream`.
4. Accumulate text, reasoning, and tool-call fragments.
5. When the finish reason is `ToolCalls`, do this for each call:
   1. Check the policy. See `A4`.
   2. Emit `ConfirmReq` if the policy requires confirmation. Wait for the intent.
   3. Call the tool. Apply a timeout.
   4. Append a `Role::Tool` message.
6. Repeat from step 3. Do not change the model.
7. Stop at the round-trip limit. The default limit is 40. The limit is 12 on a central-processor profile.
8. At the limit, add a system message. Tell the model to summarise the state. Set `ToolChoice::None`. Take one more turn.
9. On cancellation, give each running tool 5 seconds. Then abort it.
10. Write a `Role::Tool` reply for every issued call, including cancelled calls. Set the error flag. An unanswered tool call breaks the chat template.

**Do not.**

- Do not exit on the round-trip limit. An agent that stops during an edit leaves broken files.
- Do not change the resident model during a turn.

**Verify.**

```
cargo nextest run -p dark-core turn::
cargo nextest run -p dark-core --test well_formed_history
```

**Done when.** Every tool call has a matching reply, including under cancellation.

---

### A3 · Assemble the context

**Goal.** Build a stable prefix. Keep the cache warm.

**Owns.** `crates/dark-core/src/context/`.

**Needs.** `Z1`, `B1`.

**Do.**

1. Build the prefix in this order. Do not change the order.

   | # | Content | Budget at 32k |
   | --- | --- | --- |
   | 1 | System prompt | 800 |
   | 2 | AGENTS.md chain | 1500 |
   | 3 | Environment block. Date only. No time. | 100 |
   | 4 | Map digest, if a map is loaded | 1200 |
   | 5 | Claimed ticket body | 400 |

2. Build the tail in this order:

   | # | Content | Budget at 32k |
   | --- | --- | --- |
   | 6 | Tool schemas | 1200 |
   | 7 | Lexicon chunks for this turn | 4000 |
   | 8 | Message history, oldest first | 18000 |
   | 9 | The input message | (in the 18000) |
   | 10 | Tool results, appended as they arrive | (in the 18000) |
   | 11 | Reserve for generation | 4800 |

3. Compute `prefix_hash` at the start of each turn. Emit a notice when it changes. Name the cause.
4. Compact at 75% of `granted_context`. Compact only at a turn boundary.
5. To compact, fold the oldest third of unpinned history into one summary message. Use the scout micro-role. Preserve these items:
   - the files that the session changed;
   - the decisions that the session made;
   - the errors that the session met;
   - the work that remains.
6. Emit a notice when the harness compacts. Silent compaction destroys trust.
7. Evict Lexicon chunks before history. Drop them whole. Do not compact them. Retrieve them again next turn if needed.
8. Count tokens with `Engine::tokenize`.

**Do not.**

- Do not estimate tokens by character count.
- Do not put a clock in the environment block.
- Do not insert content into the prefix during a turn.

**Verify.**

```
cargo nextest run -p dark-core context::
cargo nextest run -p dark-core --test prefix_stability
cargo nextest run -p dark-core --test budget_200_turns
```

**Done when.** The first N tokens are identical across five round-trips in one turn. A 200-turn session stays inside the budget and keeps all pinned content.

---

### A4 · Add the permission policy

**Goal.** Gate mutating actions. Show the person what will happen.

**Owns.** `crates/dark-core/src/policy/`.

**Needs.** `Z1`.

**Do.**

1. Read this configuration:
   ```toml
   [policy]
   read  = "allow"
   write = "confirm"          # allow | confirm | deny
   exec  = "confirm"
   write_outside_root = "deny"
   default_dark = false
   ```
2. Emit `ConfirmReq` for a `confirm` action. Block until the intent arrives.
3. Show the exact unified diff or the exact command. Do not show a summary.
4. In headless mode, treat `confirm` as `deny`. The `--yes` flag changes this.
5. Deny `write_outside_root` always. No configuration overrides it. See Rule 34.

**Verify.**

```
cargo nextest run -p dark-core policy::
```

**Done when.** Every policy value behaves correctly. `write_outside_root` cannot be enabled.

---

## C — Tools

### C1 · Build the file tools

**Goal.** Read and change files safely.

**Owns.** `crates/dark-tools/src/fs/`.

**Needs.** `Z1`.

**Do.**

1. Build these tools:

   | Tool | Tier | Mutating |
   | --- | --- | --- |
   | `read_file` | 1 | No |
   | `write_file` | 1 | Yes |
   | `edit_file` | 1 | Yes |
   | `apply_patch` | 2 | Yes |
   | `list_dir` | 1 | No |
   | `glob` | 2 | No |

2. Limit `read_file` to 2000 lines for each call. Accept an offset and a limit.
3. Require a prior read in this session before `write_file` changes a file.
4. Require an exact single match for `edit_file`. When the match count is zero, return the three nearest candidates with their line numbers. This turns a failure into a recoverable turn.
5. Write to a temporary file. Then rename it. Preserve the file mode.
6. Refuse to write a file that changed on disk since the session read it. Report the change. Do not overwrite it.
7. Reject a hunk failure in `apply_patch`. Apply all hunks or none.
8. Attach a unified diff to every mutating result.

**Do not.**

- Do not accept a path outside the repository root.
- Do not follow a symbolic link that leaves the repository root.

**Verify.**

```
cargo nextest run -p dark-tools fs::
cargo nextest run -p dark-tools --test path_traversal
```

**Done when.** The traversal suite rejects `../`, absolute paths, escaping symbolic links, and Windows UNC paths.

---

### C2 · Build the search tools

**Goal.** Find text and files in the repository.

**Owns.** `crates/dark-tools/src/search/`.

**Needs.** `Z1`.

**Do.**

1. Use the `grep` and `ignore` crates. These crates implement ripgrep. They give correct gitignore behaviour.
2. Run in process. Do not start a subprocess.
3. Cap the result count. Report truncation.

**Verify.**

```
cargo nextest run -p dark-tools search::
```

**Done when.** Search results match `git grep` on the fixture repository, with gitignore rules applied.

---

### C3 · Build the command tool

**Goal.** Run a shell command. Stop it reliably.

**Owns.** `crates/dark-tools/src/exec/`.

**Needs.** `Z1`.

**Do.**

1. Use `tokio::process`.
2. Split the command. Run it directly. Do not pass it to a shell.
3. Require an explicit `shell = true` argument to use a shell. Raise a louder confirmation for this case.
4. Set the working directory to the repository root or below.
5. Apply a timeout. The default is 120 seconds.
6. Kill the process group on timeout. Use `killpg` on Unix. Use a Job Object on Windows.
7. Cap the output at 30000 characters. Keep the head and the tail. Insert an elision marker.
8. Emit `ToolProgress` for each output line. A silent 90-second test run looks like a failure.
9. Set `DARK_OFFLINE=1` in dark mode. On Linux, run the child in an empty network namespace where the platform permits it.

**Verify.**

```
cargo nextest run -p dark-tools exec::
cargo nextest run -p dark-tools --test process_group_kill
```

**Done when.** A timeout kills the whole process group. Output streams line by line.

---

### C4 · Gate the tools by model size

**Goal.** Show a small model fewer tools.

**Owns.** `crates/dark-tools/src/registry.rs`.

**Needs.** `Z1`, `C1`, `C2`, `C3`.

**Do.**

1. Read `Caps::params_b`. Apply these rules:

   | Model size | Tiers | Maximum tools | Tool calls each turn |
   | --- | --- | --- | --- |
   | Below 8B | 1 | 5 | 1 |
   | 8B to 32B | 1 and 2 | 12 | Many |
   | Above 32B | 1, 2, and 3 | All | Many |

2. Resolve the tool set at the start of a session.
3. Read `tools.tier_override` from the configuration.

**Do not.**

- Do not change the tool set during a turn. Tool schemas sit in the prefix. See Rule 5.

**Verify.**

```
cargo nextest run -p dark-tools registry::
```

**Done when.** A 4B capability set produces exactly the tier 1 tools.

---

## D — Cartograph

### D1 · Build the journal and the database

**Goal.** Store maps. Survive a crash. Merge in Git.

**Owns.** `crates/dark-cartograph/src/store/`, `crates/dark-cartograph/src/journal/`.

**Needs.** `Z1`.

**Do.**

1. Write an append-only event log to `$DARK_HOME/maps/<map-id>/journal.jsonl`. This file is the source of truth. Commit it to Git.
2. Build a SQLite database at `<repo>/.dark/cartograph.db` by replay. This file is derived. Do not commit it.
3. Use `rusqlite` with the `bundled` feature.
4. Add `journal.jsonl merge=union` to `.gitattributes`. Two sessions then merge by append.
5. Create this schema:
   ```sql
   CREATE TABLE maps (
     id TEXT PRIMARY KEY, name TEXT NOT NULL, destination TEXT NOT NULL,
     notes TEXT, created_at INTEGER, updated_at INTEGER,
     status TEXT CHECK(status IN ('charting','active','complete','abandoned'))
   );

   CREATE TABLE tickets (
     id TEXT PRIMARY KEY, map_id TEXT NOT NULL REFERENCES maps(id),
     name TEXT NOT NULL,
     question TEXT NOT NULL,
     type TEXT CHECK(type IN ('research','prototype','grilling','task')),
     hitl INTEGER NOT NULL,
     status TEXT CHECK(status IN
       ('open','claimed','resolved','out_of_scope','invalidated')),
     claimed_by TEXT, claimed_at INTEGER,
     resolution TEXT, gist TEXT,
     created_at INTEGER, resolved_at INTEGER,
     ordinal INTEGER NOT NULL,
     axis TEXT,
     tokens_used INTEGER
   );

   CREATE TABLE edges (
     blocker TEXT NOT NULL REFERENCES tickets(id),
     blocked TEXT NOT NULL REFERENCES tickets(id),
     PRIMARY KEY (blocker, blocked)
   );

   CREATE TABLE fog (
     id TEXT PRIMARY KEY, map_id TEXT NOT NULL,
     patch TEXT NOT NULL, axis TEXT,
     created_at INTEGER, graduated_to TEXT
   );

   CREATE TABLE scope_exclusions (
     id TEXT PRIMARY KEY, map_id TEXT NOT NULL,
     gist TEXT NOT NULL, reason TEXT NOT NULL, ticket_id TEXT
   );

   CREATE TABLE assets (
     id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL,
     kind TEXT, path TEXT, note TEXT
   );
   ```
6. Check reachability before an edge insert. Reject a cycle. Report the path. A cycle stops the frontier permanently.
7. Add `dark map rebuild`. It rebuilds the database from the journal.

**Verify.**

```
cargo nextest run -p dark-cartograph store::
cargo nextest run -p dark-cartograph --test journal_replay
```

**Done when.** Replay reproduces the database exactly. A five-node cycle is rejected with its path.

---

### D2 · Compute the frontier and manage claims

**Goal.** Show what a session can take now. Prevent two sessions from taking the same ticket.

**Owns.** `crates/dark-cartograph/src/frontier.rs`.

**Needs.** `Z1`, `D1`.

**Do.**

1. Use this query:
   ```sql
   WITH blocked AS (
     SELECT e.blocked AS id FROM edges e JOIN tickets t ON t.id = e.blocker
     WHERE t.status NOT IN ('resolved','out_of_scope')
   )
   SELECT * FROM tickets
   WHERE map_id = ?1 AND status = 'open' AND id NOT IN (SELECT id FROM blocked)
   ORDER BY ordinal;
   ```
2. Claim a ticket before any work starts. Set the status to `claimed`.
3. Give each claim a lease. The default is 2 hours.
4. Return an expired claim to the frontier. Emit a notice. Without a lease, a crashed session holds a ticket forever.

**Verify.**

```
cargo nextest run -p dark-cartograph frontier::
cargo nextest run -p dark-cartograph --test concurrent_claim
```

**Done when.** Eight parallel claims produce no double claim. An expired lease returns the ticket.

---

### D3 · Build the map digest

**Goal.** Give the model the whole map in 1200 tokens or fewer.

**Owns.** `crates/dark-cartograph/src/digest/`.

**Needs.** `Z1`, `D1`, `D2`.

**Do.**

1. Produce this format:
   ```
   MAP: Offline pack format          [active · 23 tickets · 11 resolved]

   DESTINATION
     A frozen, versioned pack format that ships vendor docs offline and can
     be verified without network access.

   NOTES
     Domain: Rust, content-addressed storage. Consult /explore before edits.

   DECISIONS SO FAR (11)
     T-004 Pack identity is content-addressed  → blake3 of canonical manifest
     T-007 Chunking is heading-aware           → h2 boundaries, 512tok target
     T-011 Embeddings live in the pack         → local embedder, dim 1024
     … 8 more · zoom with ticket_zoom(id)

   FRONTIER (3 takeable now)
     T-018 [grilling·HITL]  How does a pack declare its staleness policy?
     T-019 [research·AFK]   What does the registry return for an ambiguous name?
     T-021 [task·AFK]       Generate the fixture corpus for round-trip tests

   BLOCKED (6)
     T-020 ← T-018 · T-022 ← T-019,T-021 · … 4 more

   NOT YET SPECIFIED (fog)
     · How packs are distributed — registry or plain tarball. Blocked on T-018.
     · Whether reranking needs its own pack metadata.

   OUT OF SCOPE (2)
     · Pack signing and trust chain — separate effort (T-009, closed)
   ```
2. Compress in this sequence when the digest is too large:
   1. Collapse decisions after the ten most recent to a count.
   2. Collapse blocked tickets to edge notation. Then collapse to a count.
   3. Truncate each fog patch to its first sentence.
3. Never compress the frontier. The frontier is the actionable part.
4. Support three digest tiers:

   | Tier | Content | Use for |
   | --- | --- | --- |
   | `Full` | Everything above | Ticket resolution turns. |
   | `FrontierOnly` | Destination, notes, frontier. About 300 tokens. | Charting stages that need orientation. |
   | `None` | Nothing | Charting stages 4 to 7. Resolution recording. |

5. Show a ticket name first. Show the identifier in secondary style. A list of bare numbers is unreadable.

**Verify.**

```
cargo nextest run -p dark-cartograph digest::
cargo nextest run -p dark-cartograph --test digest_budget
```

**Done when.** A map with 500 tickets produces a digest of 1200 tokens or fewer under the real tokenizer.

---

### D4 · Build the ticket tools

**Goal.** Let the model change the map. Enforce the wayfinder rules.

**Owns.** `crates/dark-cartograph/src/tools/`.

**Needs.** `Z1`, `D1`, `D2`, `D3`.

**Do.**

1. Build these tools:
   ```
   map_read(map_id?, tier)              Return the digest.
   map_create(name, destination, notes) Return a map identifier.
   ticket_create(map_id, name, question, type, axis)
   ticket_claim(ticket_id)
   ticket_zoom(ticket_id)               Return the body, resolution, assets.
   ticket_resolve(ticket_id, resolution, gist, tokens_used)
   ticket_block(blocker, blocked)
   ticket_invalidate(ticket_id, reason)
   fog_write(map_id, patch, axis)
   fog_graduate(fog_id, ticket_ids[])
   scope_exclude(map_id, gist, reason, ticket_id?)
   ```
2. Make `ticket_resolve` one transaction. It records the answer, closes the ticket, and adds the gist to the decision index. A resolved ticket that is not indexed is invisible to the next session.
3. Enforce Rule 19. Return `E_HITL_REQUIRES_HUMAN` when the session has no human-present token.
4. Enforce Rule 20. Return `E_SESSION_RESOLUTION_LIMIT` on a second non-research resolution.
5. Record `tokens_used` on every resolution. Task unit `E6` uses this field.
6. Clear a fog patch when it graduates. The patch then exists only as its tickets.

**Verify.**

```
cargo nextest run -p dark-cartograph tools::
cargo nextest run -p dark-cartograph --test hitl_guard
```

**Done when.** Both guards return their errors. Resolution is atomic.

---

### D5 · Add map export and health

**Goal.** Move a map to a shared tracker. Show ticket sizing quality.

**Owns.** `crates/dark-cartograph/src/export/`, `crates/dark-cartograph/src/health.rs`.

**Needs.** `Z1`, `D1`, `D4`.

**Do.**

1. Add `dark map export --format=github|markdown|mermaid`.
2. For GitHub, create a parent issue with the label `wayfinder:map`. Create child issues. Use native blocking relations.
3. State in the documentation that export is one way in version 1.
4. Add `dark map health`. Show:
   - the distribution of `tokens_used` across resolved tickets;
   - each ticket that caused compaction during its resolution;
   - the ratio of research tickets to grilling tickets;
   - each axis that produced no ticket.

**Verify.**

```
cargo nextest run -p dark-cartograph export::
cargo run -p dark-cli -- map health --map <fixture>
```

**Done when.** Export produces valid output in all three formats. Health reports the four items.

---

## F — Compass

### F1 · Discover and parse files

**Goal.** Read the repository. Build syntax trees. Produce identical results every time.

**Owns.** `crates/dark-explore/src/discover/`, `crates/dark-explore/src/syntax/`.

**Needs.** `Z1`.

**Do.**

1. Walk from the repository root. Use the `ignore` crate.
2. Exclude these items:
   - files that `.gitignore` excludes;
   - files that `.darkignore` excludes;
   - files larger than 1 MB;
   - binary files. Test for a NUL byte in the first 8 KB.
   - vendored paths. The default list is `vendor/`, `node_modules/`, `third_party/`.
3. Sort paths with a byte comparator. See Rule 30.
4. Parse with the `tree-sitter` crate.
5. Support these grammars: Rust, Go, TypeScript, TSX, JavaScript, Python, Java, C#, Ruby, C, C++, SQL, Markdown.
6. Cache by tree hash. Sub-cache each file by blob hash. An incremental run then parses only changed files.
7. Parse in parallel with `rayon`.

**Verify.**

```
cargo nextest run -p dark-explore discover::
cargo nextest run -p dark-explore --test gitignore_conformance
cargo bench -p dark-explore parse
```

**Done when.** Gitignore behaviour matches `git check-ignore`, including negation patterns. A cold run on 100k lines takes less than 5 seconds. A warm run takes less than 200 milliseconds.

---

### F2 · Extract symbols and build graphs

**Goal.** Find definitions and references. Build the dependency graphs.

**Owns.** `crates/dark-explore/src/extract/`, `crates/dark-explore/src/graph/`.

**Needs.** `Z1`, `F1`.

**Do.**

1. Write a `tags.scm` query for each grammar. Produce `(kind, name, range, scope)`.
2. Extract these items for each file:
   - `imports[]`, resolved to repository paths where possible;
   - `defs[]` with `{name, kind, range, exported, doc_present, is_interface_like}`;
   - `refs[]` with `{name, range, resolved_to?}`.
3. Resolve names lexically. Use tree-sitter scopes inside a file. Use import maps and exact name match across files.
4. Record an unresolved reference as unresolved. Do not guess.
5. Attach `resolution_confidence` to each edge. The values are `exact`, `import_scoped`, and `name_only`.
6. Build three graphs with `petgraph`:
   - **F-graph.** File to file. An edge exists when one file imports another.
   - **S-graph.** Symbol to symbol. An edge exists when one definition references another.
   - **M-graph.** The F-graph contracted by directory.
7. Assign integer node identifiers in sorted path order.

**Do not.**

- Do not build a type checker. Lexical resolution is sufficient.
- Do not report a guessed reference as resolved.

**Verify.**

```
cargo nextest run -p dark-explore extract::
cargo nextest run -p dark-explore --test golden_sexpr
```

**Done when.** Each grammar matches its committed S-expression snapshot.

---

### F3 · Compute metrics and seams

**Goal.** Find the places where a change has a bounded effect.

**Owns.** `crates/dark-explore/src/seam/`.

**Needs.** `Z1`, `F2`.

**Do.**

1. Compute these values for each node:
   ```
   Ca = in-degree
   Ce = out-degree
   I  = Ce / (Ca + Ce)          I = 0 when both are 0
   A  = interface-like defs / total defs
   D  = |A + I - 1|
   ```
2. Define `is_interface_like` for each language in its grammar adapter:

   | Language | Interface-like items |
   | --- | --- |
   | Rust | `trait` |
   | Go | `interface` |
   | TypeScript | `interface`, object-shape `type`, `abstract class` |
   | Python | `Protocol`, ABC subclass |
   | Java, C# | `interface`, `abstract class` |

3. Find bridges and articulation points with Tarjan. A bridge is a hard seam.
4. Find communities with Louvain. Set the seed to 0. Visit nodes in sorted identifier order. Set the resolution to 1.0. Stop after 100 passes. Record the modularity. Louvain depends on visit order. The fixed order gives reproducibility.
5. Compute edge betweenness with Brandes. Above 5000 files, sample sources deterministically. Take every k-th node by sorted identifier. Choose k to give about 1000 sources. Record `betweenness_sampled` and `k`.
6. Compute co-change coupling from `git log --numstat -n <window>`. The default window is 500 commits. Include the window in the configuration hash.
   ```
   C(a,b) = commits that touch both / commits that touch either
   ```
7. Score each edge `e = (u → v)`:
   ```
   seam(e) = 0.35 × B(e)         normalised edge betweenness
           + 0.25 × X(e)         1 when e crosses a community boundary
           + 0.20 × A(v)         abstractness of the target
           + 0.10 × (1 - C(e))   inverse co-change
           + 0.10 × T(e)         fraction of {u,v} that tests reference
   ```
   Normalise `B` and `C` by minimum and maximum across all edges. Keep every term in the range 0 to 1. Read the weights from the configuration. Include them in the configuration hash.

   The co-change term matters. A boundary whose two sides always change together is a poor seam, even when its structure looks clean.
8. Report the highest-scoring N edges. Report every bridge, whatever its score. Mark a bridge with `hard: true`.
9. Compute the blast radius for a symbol set `S`:
   ```
   R(S)         = reverse-reachable set of S in the S-graph
   R_bounded(S) = the same traversal, stopped at any edge with seam(e) >= 0.6
   ```
   Report both sizes and the bounding seams. The ratio is the useful number. A large `R` with a small `R_bounded` means a seam already limits the change.

**Verify.**

```
cargo nextest run -p dark-explore seam::
cargo nextest run -p dark-explore --test seam_ranking_fixture
cargo nextest run -p dark-explore --test blast_radius_fixture
```

**Done when.** The fixture repository ranks its two known seams above its known poor boundary. The blast radius matches the hand-computed set.

---

### F4 · Write the output and lock it

**Goal.** Produce byte-identical output. Prove it in continuous integration.

**Owns.** `crates/dark-explore/src/output/`.

**Needs.** `Z1`, `F3`.

**Do.**

1. Write `.dark/explore/<tree-sha>.json`:
   ```json
   {
     "version": 1,
     "tree_sha": "…", "config_hash": "…",
     "stats": {"files": 812, "defs": 5140, "edges_f": 1903,
               "edges_s": 18422, "modularity": 0.61,
               "betweenness_sampled": false},
     "modules": [{"path":"crates/dark-lexicon","files":34,"Ca":6,"Ce":11,
                  "I":0.65,"A":0.21,"D":0.14,"community":3}],
     "seams": [{"from":"dark-core::turn","to":"dark-contract","score":0.81,
                "hard":false,"betweenness":0.93,"crosses_community":true,
                "abstractness_target":1.0,"cochange":0.08,
                "test_proximity":0.7}],
     "bridges": [{"from":"dark-tui::app","to":"dark-contract","hard":true}],
     "hotspots": [{"path":"crates/dark-core/src/session.rs","Ca":41,
                   "D":0.72,"churn":88}],
     "unresolved_refs": 214
   }
   ```
2. Write `.dark/explore/<tree-sha>.lock` with `{tool_version, config_hash, grammar_versions, output_blake3}`.
3. Exclude the generation time from the hashed payload. See Rule 31.
4. Add a continuous integration check. It runs the analysis on the fixture repository twice. It asserts the hashes match. It also compares the hash across operating systems.

**Verify.**

```
cargo nextest run -p dark-explore --test determinism
cargo xtask explore-fixture --assert-hash
```

**Done when.** Two runs produce identical bytes. Runs on Linux, macOS, and Windows produce identical bytes. A shuffled input order produces identical bytes.

---

### F5 · Add the narration stage

**Goal.** Explain the numbers. Do not invent numbers.

**Owns.** `crates/dark-explore/src/narrate.rs`.

**Needs.** `Z1`, `B1`, `F4`.

**Do.**

1. Give the model the JSON output or a budgeted extract of it.
2. Instruct the model to name the JSON field for each figure it states.
3. Mark the narration as model-generated in the transcript. Show it beside the numbers.
4. Run a linter on the narration. Flag any symbol that the JSON does not contain.

**Do not.**

- Do not let the narration replace the JSON. The JSON is the record.

**Verify.**

```
cargo nextest run -p dark-explore narrate::
```

**Done when.** The linter flags an invented symbol in a test fixture.

---

## G — Lexicon

### G1 · Define the pack format

**Goal.** Store one library's documentation in a portable, verifiable directory.

**Owns.** `crates/dark-lexicon/src/pack/`.

**Needs.** `Z1`.

**Do.**

1. Create this directory layout:
   ```
   packs/tokio@1.47.0/
   ├── pack.toml
   ├── chunks.jsonl
   ├── bm25.idx
   ├── dense.vec
   ├── graph.json
   └── LICENSE
   ```
2. Write this manifest:
   ```toml
   [pack]
   name = "tokio"; version = "1.47.0"; ecosystem = "crates.io"
   aliases = ["tokio-rs"]
   [source]
   kind = "docsrs"
   url  = "https://docs.rs/tokio/1.47.0/tokio/"
   etag = ""; commit = ""
   [ingest]
   at = 2026-08-19T11:03:00Z
   tool_version = "1.0.0"; chunker = "heading-v1"; chunks = 3104
   [embed]
   model = "Qwen/Qwen3-Embedding-0.6B"; dim = 1024; quant = "int8"
   query_prefix = "Instruct: retrieve documentation\nQuery: "
   doc_prefix = ""
   [staleness]
   policy = "90d"
   [license]
   spdx = "MIT"; notice_required = true
   ```
3. Support a single-file form. Use a zstd tarball with the extension `.darkpack`.
4. Verify the pack hash before use.
5. Detect an embedding model change. Compare the `[embed]` block against the current configuration. Report a mismatch. Serve lexical results until the pack is indexed again.

**Verify.**

```
cargo nextest run -p dark-lexicon pack::
cargo nextest run -p dark-lexicon --test pack_roundtrip
```

**Done when.** Export and import produce identical chunk identifiers and index hashes.

---

### G2 · Build the source adapters

**Goal.** Convert many documentation sources into one document type.

**Owns.** `crates/dark-lexicon/src/ingest/`.

**Needs.** `Z1`, `G1`.

**Do.**

1. Produce `Document { path, title, headings, body, url }` from each adapter.
2. Build these adapters:

   | Adapter | Source |
   | --- | --- |
   | `llms-txt` | An `llms.txt` or `llms-full.txt` file. Prefer this adapter. The content is already shaped for an agent. |
   | `docsrs` | `cargo doc --output-format json`. Use this for Rust crates. It gives structured items and needs no HTML parsing. |
   | `sitemap` | A sitemap and HTML pages. |
   | `git` | A repository at a tag. |
   | `localdir` | A local directory. Use this for private documentation. |
   | `openapi` | An OpenAPI document. Produce one document for each operation. |
   | `manpage` | Manual pages. |

3. Fetch through `dark-airlock`. Obey `robots.txt`. Limit the rate to 2 requests each second for one host.
4. Refuse a source with no discoverable licence. See Rule 26.
5. Treat fetched HTML as untrusted. Do not run scripts. Apply size caps and timeouts.

**Verify.**

```
cargo nextest run -p dark-lexicon ingest::
cargo nextest run -p dark-lexicon --test licence_gate
```

**Done when.** Each adapter produces documents from its fixture. A source with no licence is refused.

---

### G3 · Build the chunker

**Goal.** Split documents into retrievable parts. Produce the same parts every time.

**Owns.** `crates/dark-lexicon/src/chunk/`.

**Needs.** `Z1`, `G2`.

**Do.**

1. Name this algorithm `heading-v1`. Record the name in the manifest.
2. Split on Markdown headings. Start with the deepest heading.
3. Target 512 tokens. Set the maximum at 900. Set the minimum at 80.
4. Merge a chunk below the minimum into its next sibling.
5. Never split a fenced code block. When a code block exceeds the maximum, make it one chunk. Mark it `oversize`.
6. Attach a breadcrumb to each chunk, for example `tokio › runtime › Builder › worker_threads`.
7. Attach the source URL with its anchor.
8. Put the breadcrumb at the start of the text that the harness embeds. A breadcrumb carries most of the retrievable signal in documentation.
9. Compute `chunk_id = blake3(pack_id ‖ breadcrumb ‖ ordinal)`.
10. Count tokens with `Engine::tokenize`.

**Verify.**

```
cargo nextest run -p dark-lexicon chunk::
cargo nextest run -p dark-lexicon --test fence_integrity
```

**Done when.** The same input produces the same chunk identifiers on every platform. No chunk contains an unbalanced code fence.

---

### G4 · Build the indexes and retrieval

**Goal.** Find the right chunks. Work without an embedding model when necessary.

**Owns.** `crates/dark-lexicon/src/index/`, `crates/dark-lexicon/src/retrieve/`.

**Needs.** `Z1`, `B1`, `G3`.

**Do.**

1. Build a BM25 index. Set `k1 = 1.2` and `b = 0.75`.
2. Tokenise for code. Split camel case and snake case. Also keep the original token. `worker_threads` must match `worker_threads` and `worker threads`.
3. Do not stem identifiers. Apply light stemming to prose.
4. Encode postings with delta encoding and variable-length integers.
5. Build a dense index. Quantise each vector to int8 with an f32 scale.
6. Scan the dense index by brute force. Do not build an approximate index. A 50000-chunk pack at 1024 dimensions is about 51 MB. A full scan takes tens of milliseconds.
7. Fuse the two result lists with Reciprocal Rank Fusion:
   ```
   score = sum over lists of 1 / (60 + rank)
   ```
   Fusion needs no score calibration. BM25 scores and cosine scores are not comparable.
8. Rerank the top 50 fused results when `Caps::logprobs` is true. Measure the latency. Disable reranking by default when the latency exceeds 400 milliseconds.
9. Fill the caller's token budget. Remove duplicates by breadcrumb prefix. Include the breadcrumb and the URL in each returned chunk.
10. Enforce Rule 28. Cap one chunk at 400 tokens. Cap one response at 15% of one source document.

**Verify.**

```
cargo nextest run -p dark-lexicon index::
cargo nextest run -p dark-lexicon --test retrieval_quality
```

**Done when.** Recall at 5 is 0.8 or higher with the lexical index alone. Recall at 5 is 0.9 or higher with both indexes. The lexical bar must pass on its own. The lexical index is the fallback.

---

### G5 · Build the documentation tools and commands

**Goal.** Let the model search packs. Let the person manage packs.

**Owns.** `crates/dark-lexicon/src/tools/`, `crates/dark-lexicon/src/cli.rs`.

**Needs.** `Z1`, `G4`.

**Do.**

1. Build these tools:
   ```
   docs_resolve(query)
     -> [{pack_id, name, version, confidence, why}]

   docs_get(pack_id, topic, tokens = 4000)
     -> { snippets: [{text, breadcrumb, url, chunk_id}],
          pack: {name, version, ingested_at, age_days, stale},
          tiers_used: ["bm25","dense","rerank"] }
   ```
2. Return ambiguity from `docs_resolve`. Do not resolve it. Three candidates are better than one wrong answer.
3. Put the staleness warning in the returned text. Do not put it only in the metadata. The model reads the text.
4. Build these commands:
   ```
   dark pack add tokio --source docsrs --version 1.47.0
   dark pack add ./internal-docs --name acme-platform --version 2026.8
   dark pack list
   dark pack refresh --all
   dark pack export tokio@1.47.0 -o tokio.darkpack
   dark pack import tokio.darkpack
   dark pack reindex --all
   ```

**Verify.**

```
cargo nextest run -p dark-lexicon tools::
cargo run -p dark-cli -- pack list
```

**Done when.** A stale pack shows its warning inside the returned snippet text.

---

## K — Instruction files

### K1 · Resolve the AGENTS.md chain

**Goal.** Read the project's agent instructions. Keep the prefix stable.

**Owns.** `crates/dark-agentsmd/`.

**Needs.** `Z1`.

**Do.**

1. Resolve in this order. A later file wins a conflict.
   1. `~/.darkharness/AGENTS.md`
   2. `<repo-root>/AGENTS.md`
   3. Each directory between the root and the working directory.
2. Support `AGENTS.override.md`. An override file replaces everything above it. It does not extend it. Mark this feature as non-portable in the documentation.
3. Fall back to `CLAUDE.md`, then `GEMINI.md`, when no `AGENTS.md` exists. Read these files. Never write to them.
4. Resolve the chain at the start of a turn. Build the working set from:
   - the scope of the claimed ticket;
   - the paths in the input message;
   - the paths that the previous turn changed.
5. Put the resolved chain in the prefix. See Rule 22.
6. Put a nested file that the harness finds during a turn in the tail. Emit a notice that names the file and its subtree. See Rule 23.
7. Apply the token budget:
   ```toml
   [agents_md]
   enabled = true
   budget_tokens = 1500
   on_overflow = "truncate-warn"
   fallback_names = ["CLAUDE.md", "GEMINI.md"]
   honour_overrides = true
   follow_imports = false
   ```
8. On overflow, apply this sequence. Report each step:
   1. Drop the nested file that is furthest from the working set.
   2. Truncate the root file at a heading boundary.
   3. Emit a warning that names each file and its token count.
9. Never truncate inside a code fence. Never truncate mid-sentence.
10. Give the map notes higher precedence than AGENTS.md. AGENTS.md is repository policy. Map notes are effort policy. The narrower scope wins.
11. Give the person's message higher precedence than every file.

**Do not.**

- Do not create a `DARK.md` file. Use the existing standard.
- Do not follow imports by default. An import is an unbounded token cost and a path-traversal risk.

**Verify.**

```
cargo nextest run -p dark-agentsmd resolve::
cargo nextest run -p dark-agentsmd --test prefix_stability
```

**Done when.** A turn that touches three subtrees produces an identical prefix across its round-trips. The two late subtrees appear in the tail.

---

### K2 · Extract the machine-readable block

**Goal.** Let a repository set safe options. Prevent a repository from widening its own permissions.

**Owns.** `crates/dark-agentsmd/src/config_block.rs`.

**Needs.** `Z1`, `K1`.

**Do.**

1. Read a fenced block with the language tag `toml darkharness`:
   ~~~markdown
   ```toml darkharness
   [policy]
   exec = "deny"
   [tools]
   tier_override = 1
   ```
   ~~~
2. Accept only these keys:
   - `policy.read`, `policy.write`, `policy.exec` — only when the new value is more restrictive than the current value;
   - `tools.tier_override`;
   - `plan.axes.*`;
   - `agents_md.budget_tokens`.
3. Reject every other key. Report the rejected key. See Rule 35.
4. Never accept `policy.write_outside_root`. Never accept a model setting. Never accept a dark-mode setting.

**Verify.**

```
cargo nextest run -p dark-agentsmd config_block::
```

**Done when.** Every prohibited key is rejected. A permission-widening value is rejected.

---

### K3 · Add the explain command and the quality checks

**Goal.** Show the resolved chain. Warn about known problems.

**Owns.** `crates/dark-agentsmd/src/explain.rs`.

**Needs.** `Z1`, `K1`.

**Do.**

1. Add `dark agents explain`. Show each file in order. Show its token count. Show what it overrode.
2. Warn when the overlap between `AGENTS.md` and `README.md` exceeds 40%. Measure with shingle overlap. Duplicated content reduces agent quality.
3. Warn when the root file exceeds 150 lines. Suggest nested files.
4. Report the total chain token count in `dark doctor`.

**Do not.**

- Do not block on a warning.

**Verify.**

```
cargo run -p dark-cli -- agents explain
cargo nextest run -p dark-agentsmd explain::
```

**Done when.** The explain output matches its golden file. Both warnings fire on their fixtures.

---

## I — Qwen support

### I1 · Build the model profiles

**Goal.** Configure the harness for each model size.

**Owns.** `crates/dark-qwen/src/profile/`.

**Needs.** `Z1`.

**Do.**

1. Build a table keyed by model family and size. The configuration overrides it.
   ```toml
   [[qwen.profile]]
   match = "Qwen3-0.6B|Qwen3-1.7B"
   role = "scout"
   tool_tier = 1; max_tools = 5; one_tool_per_turn = true
   think_default = "off"
   force_grammar = true
   digest_budget = 600
   allow_charting = false

   [[qwen.profile]]
   match = "Qwen3-4B|Qwen3-8B"
   role = "worker"
   tool_tier = 1; max_tools = 8
   think_default = "auto"
   force_grammar = true
   allow_charting = false

   [[qwen.profile]]
   match = "Qwen3-14B|Qwen3-32B|Qwen3-Coder-30B-A3B"
   role = "worker"
   tool_tier = 2
   think_default = "auto"
   allow_charting = true

   [[qwen.profile]]
   match = "Qwen3.5-*"
   role = "architect"
   tool_tier = 3
   think_default = "on"
   allow_charting = true
   ```
2. Add the four micro-roles to every profile:
   ```toml
   [plan.roles.deliberate]
   think = "on";  temperature = 0.6; top_p = 0.95; grammar = false

   [plan.roles.extract]
   think = "off"; temperature = 0.2; grammar = true;  max_tokens = 1200

   [plan.roles.classify]
   think = "off"; temperature = 0.0; grammar = true;  max_tokens = 64

   [plan.roles.narrate]
   think = "off"; temperature = 0.4; grammar = false; max_tokens = 200
   ```
3. Refuse charting when `allow_charting` is false. Report the reason. A 4B model must not chart a map.
4. Read the real context limits from the loaded model. Do not use the values above as facts. They are examples.

**Verify.**

```
cargo nextest run -p dark-qwen profile::
```

**Done when.** Every supported model identifier resolves to one profile.

---

### I2 · Control thinking

**Goal.** Turn thinking on when it helps. Turn it off when it wastes time.

**Owns.** `crates/dark-qwen/src/think.rs`.

**Needs.** `Z1`.

**Do.**

1. Support three control methods. Detect which one the loaded chat template honours:
   - a template flag;
   - a `/think` or `/no_think` marker in the input;
   - an engine parameter.
2. Record the result in `Caps::thinking`.
3. Apply this policy for `ThinkMode::Auto`:

   | Turn purpose | Thinking |
   | --- | --- |
   | Charting conversation | On |
   | Seam narration | On |
   | Debugging | On |
   | Digest compression | Off |
   | Classification | Off |
   | A turn that only emits a tool call | Off |

   Thinking on a tool-selection turn costs hundreds of tokens and reaches the same call. Locally those tokens are seconds.
4. Strip `<think>` blocks into `Message::reasoning`. Handle a stream that ends inside a block.
5. Never send `reasoning` back to a model. Thinking is not part of the message history. It also inflates the cached prefix.

**Verify.**

```
cargo nextest run -p dark-qwen think::
cargo nextest run -p dark-qwen --test no_reasoning_outbound
```

**Done when.** No outbound request contains a reasoning field. A stream cut inside a think block is handled.

---

### I3 · Parse tool calls

**Goal.** Read a tool call from any output format. Recover from a malformed call.

**Owns.** `crates/dark-qwen/src/toolcall/`.

**Needs.** `Z1`.

**Do.**

1. Use the structured path when `Caps::native_tools` is true.
2. Otherwise run a streaming state machine. Qwen emits this form:
   ```
   <tool_call>{"name": "...", "arguments": {...}}</tool_call>
   ```
3. Handle these cases:
   - many calls in one message;
   - an unclosed tag at the end of a stream;
   - nested braces inside a string value;
   - prose before or after the block.
4. Validate each call against its JSON schema.
5. On a validation failure, return a `Role::Tool` error. Name the field. State the expected type. Do not fail the turn. A small model recovers from a named field. A small model does not recover from "invalid arguments".
6. Apply these repairs in sequence. Log each repair:
   1. Remove Markdown code fences.
   2. Unescape a double-encoded JSON string.
   3. Convert `"true"` to `true`. Convert a numeric string for a numeric field.
   4. Fill an omitted optional field with its default.
7. Never invent a required field.
8. Use grammar-constrained decoding by default for tool arguments. Local grammar constraint is cheap. It converts a retry loop into a guarantee.

**Verify.**

```
cargo nextest run -p dark-qwen toolcall::
cargo nextest run -p dark-qwen --test hermes_fuzz
```

**Done when.** 200 malformed samples produce no panic. Recoverable samples extract correctly.

---

### I4 · Set sampling and write the prompts

**Goal.** Configure generation. Give each model size the right instructions.

**Owns.** `crates/dark-qwen/src/sampling.rs`, `crates/dark-qwen/prompts/`.

**Needs.** `Z1`, `I1`.

**Do.**

1. Set these defaults. Check them against the model card for the exact checkpoint. These values change between Qwen releases.

   | Mode | Temperature | Top-p | Top-k | Min-p |
   | --- | --- | --- | --- | --- |
   | Thinking | 0.6 | 0.95 | 20 | 0 |
   | Not thinking | 0.7 | 0.8 | 20 | 0 |

2. Do not use greedy decoding in thinking mode. It causes repetition.
3. Add `presence_penalty` between 0.5 and 1.0 for a heavily quantised model that repeats.
4. Write three prompt fragments:

   | Fragment | Model size | Content |
   | --- | --- | --- |
   | Base | All | Identity, repository context, tool discipline, use names not identifiers. |
   | Compact | Below 8B | Short imperative lines. One instruction each line. No nested conditions. "Call one tool. Then stop." |
   | Full | 14B and above | Wayfinder discipline, plan-do-not-do, seam terms, the fog test. |

5. Version each prompt file. Add a golden test for its token length under each profile. Prompt growth reduces working space on every turn.
6. Extend the context with YaRN only at load time. Warn that static YaRN reduces short-context quality. Make an extended model a separate profile.

**Verify.**

```
cargo nextest run -p dark-qwen sampling::
cargo nextest run -p dark-qwen --test prompt_token_budget
```

**Done when.** Each prompt fragment stays inside its token budget for its profile.

---

## E — Plan

The `/plan` command implements the wayfinder method. A map holds decision tickets. Each ticket resolves one decision. The map is complete when nothing remains to decide.

`/plan` produces decisions. It does not produce deliverables. When you feel the pull to do the work, you have reached the edge of the map. Hand off instead.

A 32B model has specific weaknesses. Task units `E1` to `E7` address each one.

| Weakness | Cause | Task unit |
| --- | --- | --- |
| The model goes deep on one thread instead of wide across the space. | Wide thinking is hard. | `E2` |
| The model confuses "I can state this question" with "I know the answer". | The test is subtle. | `E4` |
| Tickets are too large. | The method assumes a 100k session. Ours is 18k. | `E5` |
| The model over-connects the blocking graph. | Graph construction is hard above ten nodes. | `E6` |
| Charting quality falls as the session grows. | The session accumulates its own output. | `E1` |

### E1 · Build the charting pipeline

**Goal.** Chart a map in seven stages. Give each stage a fresh context.

**Owns.** `crates/dark-plan/src/chart/mod.rs`.

**Needs.** `Z1`, `B1`, `D4`.

**Do.**

1. Run these seven stages:

   | # | Stage | Mode | Context in | Output | Micro-role |
   | --- | --- | --- | --- | --- | --- |
   | 1 | Destination | Human present | The idea, AGENTS.md, repository summary | `{destination, notes, type}` | `deliberate` |
   | 2 | Seed | No model | The repository | Seams, blast radius, module list | none |
   | 3 | Axis sweep | Human present, one turn each axis | Destination, one axis, the seed | Open decisions, or "nothing here" | `deliberate` |
   | 4 | Extract | Automatic | Stage 3 answers only | `{candidates, out_of_scope}` | `extract` |
   | 5 | Sharpen | Automatic, one candidate each call | One candidate | `ticket` or `fog` | `classify` |
   | 6 | Size | Automatic, one ticket each call | One ticket, the budget | `ok` or `split` | `classify` |
   | 7 | Wire | Automatic, one ticket each call | One ticket, all other names | Its blockers | `classify` |

2. Give each stage a fresh sub-session. A stage must not see the previous stage's transcript. This is the largest quality gain and it costs only plumbing.
3. Settle the destination first. The destination fixes the scope.
4. Write a checkpoint to the journal after each stage.
5. Support `dark map chart --resume <map-id> --from-stage <n>`. One bad generation in twelve is normal on a local model. A full restart makes the feature unusable.
6. After stage 4, test for fog. When no fog exists, stop. Tell the person that the work fits one session and needs no map.
7. After stage 7, create the tickets. Then wire the edges. A ticket needs an identifier before another ticket can reference it.
8. Then start the research sub-agents. Then write the fog. Then stop. Charting resolves no ticket.
9. Print a cost estimate before charting starts:
   ```
   Charting "offline pack format"
     10 axes, 1 turn each   ~10 generations   deliberate, thinking on
     extract and sharpen    ~14 generations   grammar-constrained
     size and wire          ~2N generations   single token
     estimated              ~4 min at 41 tok/s on qwen3-14b-q4
   ```

**Do not.**

- Do not carry a transcript between stages.
- Do not resolve a ticket during charting.

**Verify.**

```
cargo nextest run -p dark-plan chart::
cargo nextest run -p dark-plan --test stage_isolation
cargo nextest run -p dark-plan --test resume
```

**Done when.** Stage N's prompt contains no text from stage N-1's transcript. A charting run killed at stage 5 resumes and produces the same map as an uninterrupted run with the same seed.

---

### E2 · Build the axis sweep

**Goal.** Replace wide thinking with enumeration against a list.

**Owns.** `crates/dark-plan/src/axes/`.

**Needs.** `Z1`, `E1`.

**Do.**

1. Define these axis sets. The configuration and the AGENTS.md block override them.
   ```toml
   [plan.axes.spec]
   axes = [
     "data model and invariants",
     "interfaces and boundaries",
     "failure modes and error handling",
     "lifecycle, migration and backfill",
     "observability",
     "testing strategy",
     "performance envelope",
     "security and permissions",
     "dependencies and versioning",
     "rollout and reversibility",
   ]

   [plan.axes.decision]
   axes = ["options on the table", "evaluation criteria",
           "constraints that remove options", "cost to reverse",
           "who must agree", "what would show we are wrong"]

   [plan.axes.in_place]
   axes = ["current shape", "target shape", "migration path",
           "blast radius", "verification", "rollback"]
   ```
2. Select the axis set from the destination type.
3. Ask one narrow question for each axis. Ask it in its own turn.
4. Seed the axes from stage 2. The seam report answers "blast radius" and much of "current shape" with computed numbers. Give the model those numbers. Do not ask it to guess them.
5. Record which axis produced each candidate. Store it in `tickets.axis` and `fog.axis`.
6. Accept "nothing here" as a valid answer for an axis.

**Verify.**

```
cargo nextest run -p dark-plan axes::
```

**Done when.** Each axis produces either candidates or an explicit empty answer. The axis is recorded on every candidate.

---

### E3 · Build the extraction stage

**Goal.** Convert conversation into structure.

**Owns.** `crates/dark-plan/src/extract.rs`.

**Needs.** `Z1`, `E2`.

**Do.**

1. Run one generation over the stage 3 answers. Use the `extract` micro-role.
2. Constrain the output to this schema:
   ```json
   {
     "candidates": [{"name": "...", "question": "...",
                     "axis": "...", "type": "research|prototype|grilling|task"}],
     "out_of_scope": [{"gist": "...", "reason": "..."}]
   }
   ```
3. Apply these deterministic checks. Reject and retry with a repair message when a check fails:
   - each name is unique;
   - each question ends with a question mark;
   - no question restates the destination;
   - each name is 12 words or fewer;
   - the type is one of the four values;
   - at least one candidate is not a research candidate.

**Do not.**

- Do not run extraction in the same turn as the conversation. Separating them is what makes this work on a 14B model.

**Verify.**

```
cargo nextest run -p dark-plan extract::
cargo nextest run -p dark-plan --test extract_schema_rate
```

**Done when.** 20 fixture transcripts produce schema-valid output on the first attempt in 90% of cases.

---

### E4 · Build the fog classifier

**Goal.** Decide whether a candidate is a ticket or fog.

**Owns.** `crates/dark-plan/src/sharpen.rs`.

**Needs.** `Z1`, `E3`.

**Do.**

1. Test one candidate for each call. Use the `classify` micro-role. Set the temperature to 0. Constrain the output to one word.
2. Use this prompt:
   ```
   Candidate: "How does a pack declare its staleness policy?"

   Can this question be STATED precisely now?
   This does not ask whether you can answer it.
   A question can be stated precisely even when the answer needs
   research, a prototype, or a decision that nobody has made yet.

     TICKET — the question is already sharp, even when it is blocked
     FOG    — you cannot yet phrase it sharply, because it depends on
              something that is still open

   Answer with one word.
   ```
3. Add two examples. Add one for each answer.
4. Apply these exclusions with code. Do not ask the model:
   - a candidate that repeats a recorded decision is not fog;
   - a candidate that matches a live ticket name is not fog;
   - a candidate that an out-of-scope entry covers is not fog.
5. Write one fog patch for each axis. When stage 4 produced four fog candidates on one axis, merge them into one patch. Fog is coarser than a ticket. One patch may become several tickets later, or none.

**Verify.**

```
cargo nextest run -p dark-plan sharpen::
cargo nextest run -p dark-plan --test fog_classifier_accuracy
```

**Done when.** The classifier agrees with a 40-case hand-labelled set in 90% of cases.

---

### E5 · Size the tickets

**Goal.** Make each ticket fit one session. Measure the result.

**Owns.** `crates/dark-plan/src/size.rs`.

**Needs.** `Z1`, `E4`, `D4`.

**Do.**

1. Compute the budget:
   ```
   ticket_budget = granted_context × 0.55
   ```
   This gives about 18000 tokens at a 32k grant.
2. Test one ticket for each call. Use the `classify` micro-role. Ask three questions:
   - How many files does this ticket touch?
   - Does this ticket need research?
   - Does this ticket contain more than one decision?
3. Split a ticket that contains more than one decision. This signal is reliable. A model detects it more accurately than it estimates tokens.
4. Read `tickets.tokens_used` after resolutions accumulate. Calibrate the estimator.
5. Flag a ticket that caused compaction during its resolution. Report it in `dark map health`.

**Verify.**

```
cargo nextest run -p dark-plan size::
```

**Done when.** A multi-decision fixture ticket is split. Token counts appear in `dark map health`.

---

### E6 · Wire the blocking edges

**Goal.** Build a correct blocking graph from a weak model's output.

**Owns.** `crates/dark-plan/src/wire.rs`.

**Needs.** `Z1`, `E5`, `D1`.

**Do.**

1. Ask one question for each ticket. Do not construct the graph in one call.
   ```
   Ticket: "How does a pack declare its staleness policy?"
   Which of these must be answered BEFORE this question can be resolved?
     · Pack identity is content-addressed
     · What does the registry return for an ambiguous name?
     · Generate the fixture corpus for round-trip tests
   Answer with names, or NONE.
   ```
2. Apply these deterministic repairs. Most of the quality comes from this step.
   1. **Break cycles.** Drop the edge whose blocker has the higher ordinal. Report every break.
   2. **Reduce transitively.** When A blocks B and B blocks C, remove any A-to-C edge. A model asserts implied edges. This repair removes them.
   3. **Cap out-degree.** Flag a ticket that blocks more than five others. This usually indicates a parse error.
   4. **Check the frontier.** When the wired graph has an empty frontier, the wiring is wrong. Every map must start with a takeable ticket. Fail the stage. Retry it.

**Do not.**

- Do not accept a graph with an empty frontier.

**Verify.**

```
cargo nextest run -p dark-plan wire::
cargo nextest run -p dark-plan --test transitive_reduction
```

**Done when.** An over-connected 12-node fixture reduces to the correct minimal edge set. A five-node cycle breaks with a report.

---

### E7 · Work the map

**Goal.** Resolve one ticket. Clear the fog that the answer reveals.

**Owns.** `crates/dark-plan/src/work.rs`.

**Needs.** `Z1`, `D4`, `E6`.

**Do.**

1. Load the digest. Do not load every ticket body.
2. Select the ticket. Use the named ticket. Otherwise use the first frontier ticket by ordinal.
3. Claim the ticket before any work starts.
4. Resolve it. Call `ticket_zoom` on a related ticket only when needed.
5. Route by ticket type:

   | Type | Human present | Method |
   | --- | --- | --- |
   | `research` | No | A sub-session with read-only tools: `docs_*`, `grep`, `read_file`, `explore`. Prefer this type. It is bounded and needs retrieval, not long reasoning. |
   | `prototype` | Yes | Make a cheap rough artefact. Link it as an asset. Then discuss it. |
   | `grilling` | Yes | Conversation. Use the `deliberate` micro-role. |
   | `task` | Either | Do the manual work that unblocks a decision. Record what was done and the facts that later tickets need. |

6. Record the resolution in one transaction. See `D4`.
7. Graduate the fog that the answer made specifiable. Create the tickets. Then wire them. Clear each graduated patch.
8. When the answer shows that a ticket sits past the destination, close that ticket. Add one line to the out-of-scope section. Do not resolve it. A scope boundary is not a step on the route.
9. When the decision invalidates another ticket, update it or delete it.
10. Stop after one ticket. Research tickets are exempt.
11. Limit parallel sub-agents. Read the headroom from the resident set. The default is 2. Each sub-agent holds a key-value cache.

**Do not.**

- Do not resolve a second non-research ticket in one session.
- Do not start eight research sub-agents. That exhausts memory.

**Verify.**

```
cargo nextest run -p dark-plan work::
cargo nextest run -p dark-plan --test hitl_headless
cargo nextest run -p dark-plan --test subagent_memory_cap
```

**Done when.** Headless work on a grilling ticket returns `E_HITL_REQUIRES_HUMAN`. A second resolution returns `E_SESSION_RESOLUTION_LIMIT`. The sub-agent count respects a synthetic low-memory state.

---

## H — Horizon

### H1 · Build the application shell

**Goal.** Show two panes, a command bar, and a function-key bar.

**Owns.** `crates/dark-tui/src/app/`.

**Needs.** `Z1`.

**Do.**

1. Use `ratatui`, `crossterm`, and `tokio`. Pin one `crossterm` major version across the workspace. Two versions produce confusing type errors.
2. Subscribe to both event channels. Send intents on an mpsc channel.
3. Build this layout:
   ```
   ┌ darkharness ─ myrepo ⎇ main ─ ◆ LOCAL qwen3-14b-q4 ─ ctx 34% ─ 41 tok/s ┐
   │ ┌─ MAP: Offline pack format ────┐ ┌─ TRANSCRIPT ──────────────────────┐ │
   │ │ ▸ FRONTIER                    │ │ ▸ thinking (312 tok) ············ │ │
   │ │   ◆ T-018 staleness policy    │ │ Reading crates/dark-lexicon/pack  │ │
   │ │   ◆ T-019 ambiguous names     │ │ ┌ edit_file · pack.rs ──────────┐ │ │
   │ │   ◆ T-021 fixture corpus      │ │ │ - fn stale(&self) -> bool {   │ │ │
   │ │ ▸ BLOCKED (6)                 │ │ │ + fn stale(&self, now) -> …   │ │ │
   │ │ ▸ DECISIONS (11)              │ │ └───────────────────────────────┘ │ │
   │ │ ▸ FOG (2)                     │ │ ⣾ cargo nextest run -p dark-lex   │ │
   │ └───────────────────────────────┘ └───────────────────────────────────┘ │
   │ ⟩ _                                                                     │
   │ 1Help 2Map 3View 4Diff 5Explore 6Lexicon 7Ticket 8Resolve 9Menu 0Quit   │
   └─────────────────────────────────────────────────────────────────────────┘
   ```
4. Cycle the left pane through Map, Files, Seams, and Packs. Cycle the right pane through Transcript, Diff, Doc, and Explore.
5. Bind these keys:
   ```
   F1 help   F2 map    F3 view   F4 diff   F5 explore
   F6 lexicon F7 ticket F8 resolve F9 menu  F10 quit
   Tab focus · Ctrl+←/→ pane mode · Ctrl+P palette · Ctrl+D dark toggle
   Esc cancel turn · Ctrl+C quit, twice during a turn
   t thinking · c claim · r resolve · f fog · / filter · ? keys
   ```
6. Show the resident set in the status bar. Show the model, the quantisation, the device, and the measured rate. Show a progress bar during a model load.
7. Support 80 columns by 24 rows. Below that size, stack the panes. Do not clip them.
8. Build a zone registry for the mouse. Map a rectangle to an identifier.

**Verify.**

```
cargo nextest run -p dark-tui app::
cargo nextest run -p dark-tui --test golden_frames
cargo nextest run -p dark-tui --test resize_fuzz
```

**Done when.** Golden frames match at 80×24, 120×40, and 200×60. Resize down to 40×10 causes no panic.

---

### H2 · Build the theme

**Goal.** Show state through colour. Make dark mode unmissable.

**Owns.** `crates/dark-tui/src/theme/`.

**Needs.** `Z1`.

**Do.**

1. Build a token layer over `ratatui::Style`. Name each token. Add a gradient helper.
2. Use this palette. The model is an accretion disk seen near edge-on. The limb that turns toward the viewer is bright and blue. The limb that turns away is dim and red.
   ```
   singularity   #05060A   application background
   horizon       #0B0D14   panel background
   photon-ring   #FFF4D6   selection, cursor, focused border
   disk-inner    #FFC15E
   disk-mid      #FF7A18   primary accent, active work
   disk-outer    #C2410C
   ember         #7C2D12   resolved, historical
   doppler-blue  #7DD3FC   approaching: frontier, takeable now
   doppler-dim   #38536B   receding: blocked, waiting
   fog           #1E293B   dithered: not yet specified
   void          #0F172A   out of scope, outside the disk
   text          #E2E8F0   text-dim #64748B
   danger        #F43F5E   ok #34D399   warn #FBBF24
   ```
3. Map each state to one token:

   | State | Token |
   | --- | --- |
   | Frontier, takeable | `doppler-blue` |
   | Claimed, in progress | `disk-mid`, pulsing |
   | Resolved | `ember`, static |
   | Blocked | `doppler-dim` |
   | Fog | `fog`, dithered |
   | Out of scope | `void`, outside the disk |
   | Dark mode | Red status bar. Desaturated disk. |
   | Model loading | `disk-inner` sweep |

4. Change the whole palette when dark mode changes. Take 400 milliseconds. The person must never be unsure about the network state.
5. Degrade colour: true colour, then 256, then 16, then none. Read `NO_COLOR` and `COLORTERM`.
6. In 16-colour mode and no-colour mode, use ASCII density characters for the fog map: ` .:-=+*#%@`.

**Verify.**

```
cargo nextest run -p dark-tui theme::
cargo nextest run -p dark-tui --test colour_degradation
```

**Done when.** Snapshots match at all four colour levels.

---

### H3 · Build the fog map

**Goal.** Show the whole map at once. Show the frontier as the brightest part.

**Owns.** `crates/dark-tui/src/views/fogmap.rs`, `crates/dark-tui/src/anim/`.

**Needs.** `Z1`, `H2`.

**Do.**

1. Use `ratatui::widgets::canvas::Canvas` with `Marker::Braille`. Braille gives 2 by 4 subpixels for each cell.
2. Compute the layout deterministically. Do not use a force simulation. The map must look identical every time.
   - Put the destination at the centre.
   - Set the radius from the longest path to the destination in the blocking graph. A resolved ticket moves inward. Fog sits at the outer edge. An out-of-scope item sits outside the disk.
   - Set the angle from a stable hash of the ticket identifier. Then relax the position inside its ring with a fixed number of passes.
3. Draw the frontier as a bright ring. The frontier is the takeable set. It must be the brightest part of the display.
4. Use these glyphs:
   ```
   ◆ open    ◈ claimed    ● resolved    · fog    × out of scope
   ```
5. Show the ticket name beside the glyph where space allows. Show the name, not the identifier.
6. Write a spring integrator for animation. Use a damped harmonic oscillator:
   ```rust
   pub struct Spring { /* four coefficients */ }
   impl Spring {
       /// dt: seconds each frame. freq: radians each second. Try 6.0.
       /// damping: 1.0 is critical. Below 1.0 bounces. Above 1.0 is slow.
       pub fn new(dt: f32, freq: f32, damping: f32) -> Self { /* … */ }
       pub fn update(&self, pos: &mut f32, vel: &mut f32, target: f32) { }
   }
   ```
7. Use the spring for camera movement, pane transitions, and the dark-mode colour change.
8. Add a slow shimmer. Apply a phase-offset sine to the luminance of each cell at 0.15 Hz.
9. Set a frame budget of 8 milliseconds. When a frame exceeds it, remove the shimmer first. Then remove the gradient. Keep the layout.
10. Redraw only changed regions.
11. Disable animation when any of these conditions is true:
    - `TERM` is `dumb`;
    - the output is not a terminal;
    - `DARK_NO_ANIM` is set;
    - the window has no focus;
    - three consecutive frames exceeded the budget.
12. Bind these keys: arrows move between and around rings, `Enter` opens the detail pane, `c` claims, `r` resolves, `f` writes fog, `/` filters.

**Verify.**

```
cargo nextest run -p dark-tui fogmap::
cargo nextest run -p dark-tui --test fogmap_determinism
cargo bench -p dark-tui fogmap_frame
```

**Done when.** The same map produces identical bytes twice. A 500-ticket map holds 30 frames each second with shimmer. A 5000-ticket map degrades without a panic.

---

### H4 · Build the transcript and diff views

**Goal.** Show output as it arrives. Show changes clearly.

**Owns.** `crates/dark-tui/src/views/transcript.rs`, `crates/dark-tui/src/views/diff.rs`.

**Needs.** `Z1`, `H2`.

**Do.**

1. Collapse reasoning by default. Show `▸ thinking (312 tok)` with a live count. Expand it with `t`. Qwen thinking output is long. Showing it buries the answer. Hiding it prevents debugging.
2. Coalesce token deltas on a 16-millisecond tick. Do not redraw for each token.
3. Show a warning glyph when the lossy channel reports a lag.
4. Render tool output with `ansi-to-tui`.
5. Highlight code with `tree-sitter-highlight`.
6. Render Markdown. Choose `termimad` or a renderer built on `pulldown-cmark`. Prototype both in the first week. Do not delay on this choice.
7. Show a unified diff for each mutating tool result.
8. Show the exact diff or the exact command in a confirmation modal. Never show a summary.

**Verify.**

```
cargo nextest run -p dark-tui views::
```

**Done when.** Golden frames match for a streaming turn, a collapsed thinking block, and a diff.

---

### H5 · Build the replay harness

**Goal.** Develop and test the interface without the engine.

**Owns.** `crates/dark-tui/src/replay.rs`.

**Needs.** `Z1`, `H1`.

**Do.**

1. Add `dark replay <session>`. Read a transcript. Drive the interface from it.
2. Support a speed multiplier and a step mode.
3. Use this harness for every golden frame test.

**Verify.**

```
cargo run -p dark-cli -- replay testdata/sessions/fixture
cargo nextest run -p dark-tui replay::
```

**Done when.** A recorded transcript reproduces the same frames every time.

---

## J — Configuration, network, and packaging

### J1 · Build the airlock

**Goal.** Make network egress auditable. Enforce dark mode at the socket.

**Owns.** `crates/dark-airlock/`.

**Needs.** `Z1`.

**Do.**

1. Expose one constructor: `Client::new(dark: bool)`.
2. In dark mode, refuse every address that is not loopback. Refuse at the connector, before the name lookup. Return `E_POLICY_DARK`.
3. Add rules to `deny.toml`. Prohibit `reqwest`, `hyper`, and `ureq` in every other crate. See Rule 13.
4. Refuse a Git operation that reaches a remote in dark mode. Name the remote in the error.
5. Set `DARK_OFFLINE=1` for every child process in dark mode.
6. State in the documentation that the child-process block is advisory on macOS and Windows.

**Verify.**

```
cargo nextest run -p dark-airlock
cargo deny check bans
```

**Done when.** A dark-mode request to a non-loopback address fails. `cargo deny` fails when another crate adds an HTTP dependency.

---

### J2 · Build the configuration system

**Goal.** Resolve settings from five sources. Show which source won.

**Owns.** `crates/dark-config/`.

**Needs.** `Z1`.

**Do.**

1. Resolve in this order. A later source wins.
   1. Built-in defaults.
   2. `$DARK_HOME/config.toml`.
   3. `<repo>/.dark/config.toml`.
   4. Environment variables with the prefix `DARK_`.
   5. Command-line flags.
2. Add `dark config explain <key>`. Show the value. Show the source that set it.
3. Store a Hugging Face token in the operating system keyring. Use the `keyring` crate. Never store it in a configuration file.

**Verify.**

```
cargo nextest run -p dark-config
cargo run -p dark-cli -- config explain policy.write
```

**Done when.** Each source overrides correctly. The explain output names the source.

---

### J3 · Build setup and doctor

**Goal.** Prepare the machine for offline work. Report what is missing.

**Owns.** `crates/dark-cli/src/setup.rs`, `crates/dark-cli/src/doctor.rs`.

**Needs.** `Z1`, `B6`, `G5`, `K3`.

**Do.**

1. Build `dark setup`. Run these steps:
   1. Run `dark tune`. Write the hardware profile.
   2. Recommend a profile. Show the model, the quantisation, the context, the expected rate, and whether the role classes share one model.
   3. Download the weights. Show the size before the download starts.
   4. Convert the weights to UQFF. This makes a later model swap fast.
   5. Verify with a live test. Run one generation. Run one tool call. Run one embedding.
   6. Detect the ecosystem. Read `Cargo.toml`, `package.json`, `pyproject.toml`, and `go.mod`. Suggest packs.
   7. Index the packs.
   8. Run `dark doctor`. Print `OFFLINE READY` or list what is missing.
2. Build `dark doctor`. Check these items. Give a remedy for each failure.

   | Check | Failure remedy |
   | --- | --- |
   | Build variant against detected accelerator | Install the matching artefact. |
   | Total memory, available memory, budget | Reduce the context or the model size. |
   | Model manifest hashes | Download the model again. |
   | Live generation and embedding | Reinstall the model. |
   | Measured rate and hardware class | State the expected turn duration. |
   | Pack hashes and staleness | Refresh the pack. |
   | Embedding model against pack manifests | Run `dark pack reindex`. |
   | Instruction chain token count | Reduce AGENTS.md. |
   | Tree-sitter grammar versions | Rebuild. |
   | Git presence | Install Git. `/explore` needs it for co-change. |
   | Terminal capability | Set `TERM` correctly. |

3. Add `dark doctor --offline`. It checks the offline path only. It exits with a non-zero code when any item needs the network.

**Verify.**

```
cargo run -p dark-cli -- setup --dry-run
cargo run -p dark-cli -- doctor
cargo run -p dark-cli -- doctor --offline
```

**Done when.** `dark doctor --offline` prints `OFFLINE READY` on a prepared machine.

---

### J4 · Build the release pipeline

**Goal.** Produce three artefacts. Make the build reproducible.

**Owns.** `.github/workflows/release.yml`, `dist-workspace.toml`, `xtask/src/release.rs`.

**Needs.** `Z1`.

**Do.**

1. Produce the three artefacts from Section 4.5.
2. Use `cargo-dist`. Use `cross` or `zig cc` for the portable build.
3. Compile the tree-sitter grammars at build time. Never at run time.
4. Pin the toolchain. Build with `--locked`. Strip symbols.
5. Embed these assets: prompts, the default configuration, the theme, and the grammars. Do not embed model weights.
6. Set a binary size limit of 80 MB for the portable build. Add a size check to continuous integration.
7. Add a `grammars-core` feature with eight languages. Make it the default. Add a `grammars-full` feature.
8. Publish a software bill of materials, checksums, and cosign signatures.
9. Make `dark update` exit without error when the network is unavailable.

**Verify.**

```
cargo dist build
cargo xtask check-binary-size
cargo xtask check-reproducible
```

**Done when.** Two builds from one commit produce identical binaries. Each artefact stays inside its size limit.

---

### J5 · Build the air-gap test

**Goal.** Prove the primary requirement. This is the most important test in the project.

**Owns.** `xtask/src/airgap.rs`, `testdata/airgap/`.

**Needs.** `A2`, `A3`, `D4`, `E7`, `F4`, `G5`, `J3`.

**Do.**

1. Build a container. Run `dark setup` inside it. Download a small model and two packs.
2. Remove the network namespace.
3. Run a scripted session. It must complete these actions:
   - chart a map with `/plan`;
   - work one research ticket;
   - run `/explore` and read the seam report;
   - retrieve documentation with `docs_get`;
   - edit a file and run a test.
4. Assert that no step reports a network error.
5. Run this test in continuous integration on every change to `main`.

**Verify.**

```
cargo xtask airgap
```

**Done when.** The scripted session completes with no network.

---

### J6 · Add telemetry and statistics

**Goal.** Measure the harness. Keep the data local.

**Owns.** `crates/dark-core/src/telemetry/`, `crates/dark-cli/src/stats.rs`.

**Needs.** `Z1`, `A1`.

**Do.**

1. Write to `$DARK_HOME/telemetry.jsonl`. Record these values:
   - turn duration;
   - tokens in and tokens out;
   - the generation rate;
   - model load count and load duration;
   - tool failure rate;
   - **prefix cache hit rate**;
   - frame budget overruns.
2. Never record prompt text. Never record file content.
3. Send no data anywhere. There is no remote sink.
4. Add `dark stats`. Render with the `ratatui` `Chart` widget.
5. Show the prefix cache hit rate first. It is the strongest predictor of perceived speed.

**Verify.**

```
cargo nextest run -p dark-core telemetry::
cargo run -p dark-cli -- stats
```

**Done when.** The telemetry file contains no prompt text. The statistics view renders.

---

## 7 Build order

| Milestone | Task units | Exit condition |
| --- | --- | --- |
| **M0** | `Z1`, `B1` | The workspace compiles. Every crate builds against the fake engine. |
| **M1** | `A1`–`A4`, `B2`–`B7`, `C1`–`C4`, `K1`–`K3` | A real model loads and streams. It calls a tool. Cancellation releases memory. The instruction chain resolves into a stable prefix. |
| **M2** | `F1`–`F5`, `G1`–`G5` | `/explore` produces identical bytes across platforms. `docs_get` returns useful results. |
| **M3** | `D1`–`D5`, `E1`–`E7`, `I1`–`I4` | A person charts a map, works one ticket, and graduates fog. |
| **M4** | `H1`–`H5` | The terminal application runs. The fog map renders. Golden frames pass. |
| **M5** | `J1`–`J6` | `cargo xtask airgap` passes. `dark doctor --offline` prints `OFFLINE READY`. |

**Note on M4.** The terminal stack needs more foundation work than a Go equivalent. Budget one to two extra weeks for the theme tokens, the Markdown renderer, and the spring integrator. The canvas widget offsets part of this cost: the fog map is easier in `ratatui` than in most alternatives.

**Note on K1.** Build the instruction chain in M1, not later. It changes the prefix. The prefix affects every other component. A later change means reworking context assembly.

---

## 8 Questions to answer during the build

Record each answer as an architecture decision record in `docs/adr/`. The project should use its own map from M3.

1. **Does the engine expose log probabilities?** If it does not, reranking stays disabled. Retrieval then uses fusion only. Measure the quality loss.
2. **Which eight languages belong in `grammars-core`?** Measure the binary size for each grammar. Do not decide by opinion.
3. **At what memory does the architect class stop sharing a model?** Collect `dark tune` results from real machines. Do not decide from the formula.
4. **Should a research sub-agent share the engine or reserve memory?** Sharing is simpler. Start with sharing.
5. **Which Markdown renderer?** Prototype `termimad` and a `pulldown-cmark` renderer in the first week of M4.
6. **Are the default axis sets correct?** Record which axes produce tickets and which produce nothing. Prune after twenty real maps.
7. **Should a map span more than one repository?** Version 1 binds a map to one repository root. Real work spans repositories. Put this question in the fog, not in a backlog.
8. **Should the harness support the Model Context Protocol?** The engine includes a client. A remote server breaks the offline requirement. Version 1 excludes it. If it lands, dark mode must block it.

---

## 9 Definition of done for the whole project

The project is complete when all of these conditions are true:

1. `cargo clippy --workspace -- -D warnings` passes.
2. `cargo nextest run --workspace` passes.
3. `cargo xtask check-deps` passes.
4. `cargo xtask airgap` passes.
5. `cargo xtask check-reproducible` passes.
6. `dark doctor --offline` prints `OFFLINE READY` on a prepared machine.
7. `/explore` produces identical bytes on Linux, macOS, and Windows.
8. Test coverage is 70% or higher overall. It is 90% or higher in `dark-explore::seam`, `dark-cartograph::store`, and `dark-qwen::toolcall`.
9. The README states the hardware floor, the security posture, and the three artefacts.
10. A person charts a map, works three tickets, and ships a change, with no network connection.

---

## Appendix A — Error codes

| Code | Meaning | Remedy to show |
| --- | --- | --- |
| `E_ENGINE_WONT_FIT` | The model does not fit in the memory budget. | Reduce the context. Use a smaller quantisation. Share a model between role classes. |
| `E_ENGINE_UNSUPPORTED` | The engine lacks a capability. | Disable the feature. Use a different model. |
| `E_POLICY_DARK` | Dark mode blocked the action. | Run `/golight` to allow the network. |
| `E_POLICY_DENIED` | The policy denied the action. | Change the policy setting. |
| `E_TOOL_NOT_FOUND` | The path does not exist. | List the directory first. |
| `E_TOOL_STALE` | The file changed on disk. | Read the file again. |
| `E_TOOL_AMBIGUOUS` | The edit string matched more than once. | Add more context to the string. |
| `E_MAP_CYCLE` | The edge creates a blocking cycle. | Remove one edge on the reported path. |
| `E_MAP_EMPTY_FRONTIER` | No ticket is takeable. | Check the blocking edges. |
| `E_HITL_REQUIRES_HUMAN` | The ticket needs a person. | Open the terminal application. Confirm in the modal. |
| `E_SESSION_RESOLUTION_LIMIT` | This session already resolved a ticket. | Start a new session. |
| `E_PACK_NO_LICENCE` | The source has no discoverable licence. | Add a licence file. Use `--i-accept-responsibility`. |
| `E_PACK_DIM_MISMATCH` | The pack vectors do not match the embedding model. | Run `dark pack reindex`. |
| `E_EXPLORE_DIRTY` | The working tree changed during analysis. | Commit or stash. Run the analysis again. |

---

## Appendix B — Token budget at a 32k grant

| Part | Tokens | Cached |
| --- | --- | --- |
| System prompt | 800 | Yes |
| AGENTS.md chain | 1500 | Yes |
| Environment block | 100 | Yes |
| Map digest | 1200 | Yes |
| Claimed ticket | 400 | Yes |
| Tool schemas | 1200 | Yes |
| Lexicon chunks | 4000 | No |
| Working space | 18000 | No |
| Generation reserve | 4800 | No |
| **Total** | **32000** | |

Three consequences:

1. The 1200-token digest limit is 4% of the context. It sits in the cache. Each extra 100 tokens removes 100 tokens from working space on every turn. Defend the limit.
2. Charting stages 4 to 7 need no digest. Load the digest by turn type.
3. Evict Lexicon chunks before history. They are the largest recoverable block. Retrieve them again next turn when needed.

---

## Appendix C — Background sources

Do not read these to complete a task. This document is complete. These sources explain where the design came from.

| Source | Contribution |
| --- | --- |
| The wayfinder method | The map, decision tickets, the fog of war, out-of-scope handling, ticket types, the human-in-the-loop rules, and the one-ticket-per-session rule. |
| The pi agent harness | The layer separation and the statement that a harness gates actions but does not contain them. |
| mistral.rs | The engine. Model builders, in-situ quantisation, GGUF and UQFF support, and the hardware-tuning pattern. |
| ratatui | The canvas widget, the chart widgets, and the crossterm version constraint. |
| tree-sitter and petgraph | Repository analysis and the graph algorithms. |
| AGENTS.md | The instruction file standard, its nesting rules, and its size guidance. |
| ASD-STE100 | The writing rules in Section 1.5. |

Model identifiers, context lengths, quantisation names, memory figures, and sampling values in this document are examples. Verify them against the model card and the loaded model. `dark tune` and `dark doctor` exist so this document does not have to stay correct about them.
