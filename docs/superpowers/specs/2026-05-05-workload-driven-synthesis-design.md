# Mycelium — Workload-Driven Synthesis (Phase 2 policy revision)

**Status**: Approved design as of 2026-05-05. Revises the Phase 2 policy shipped 2026-05-04. Phase 2's *infrastructure* (synthesizer trait, prompt builder, atomic description+embedding write) survives unchanged. What changes is *when* synthesis runs and *who* runs it.

## Why this exists

Phase 2 shipped behavioral description synthesis: for every Symbol in the graph, build a prompt from signature + body slice + 1-hop neighbors, call a local Ollama Synthesizer, persist a description, re-embed on it. This works for `mycel find` quality — descriptions cluster by behavior — but it's catastrophically slow at scale.

Concrete data point: the dogfood index of this Rust workspace took **30 minutes**. Linear extrapolation puts a typical work-scale TypeScript codebase (~50k symbols) at multiple days of indexing. The per-symbol Ollama round-trip is the bottleneck, and it doesn't matter how fast the model is — the topology of "one LLM call per symbol, every symbol" is wrong.

Cursor solves the same problem by *not* synthesizing at index time at all: chunk files, embed chunks, do understanding at query time. Their cold-index pass is embedding-bound, which is ~100x faster.

This design adopts Cursor's cold-index posture **and** preserves Phase 2's behavioral-search bet by moving synthesis off the indexing hot path and onto a workload-driven trigger: Claude Code itself, working in the codebase as part of normal user tasks, writes descriptions for the symbols it has actually read and reasoned about. The function body is already loaded in Claude's context — the description is a side-effect, not a separate retrieval. Marginal token cost is near-zero.

The upshot: Mycelium's indexer becomes Cursor-fast for cold builds. The behavioral-search advantage emerges over time, scoped to the regions of the codebase Claude actually touches — which is exactly where retrieval quality matters most.

## Scope of this revision

**In scope:**
- Replace eager-at-index synthesis with a Cursor-style cold index that embeds `signature + body slice`.
- Demote `mycel synthesize` to an opt-in bulk command. `mycel index` no longer triggers it.
- Add a new CLI command `mycel set-description <qname> <description>` that accepts text from any caller (primary caller: Claude Code), runs the embedder, and atomically writes description + embedding.
- Ship a Mycelium skill (`mycel-graph-care`) that teaches Claude *when* and *what* to synthesize during normal work, and *how* to write back via the new CLI.
- Add a `description_source_hash` field on the Symbol schema so the daemon can invalidate stale descriptions when the source body changes.
- Update `CLAUDE.md` and `DESIGN.md` to reflect the new policy.

**Explicitly out of scope:**
- Changing the Synthesizer trait surface or the existing Ollama / future cloud impls. They remain the bulk-pass backend for users without Claude Code.
- Reranking, query-time on-demand synthesis on low-confidence `find` hits — this stays Phase 3 work.
- Cross-file CALLS / IMPLEMENTS / TYPED_BY edge resolution — separately tracked Phase 2/3 work.
- Multi-repo daemon / SKILL.md as a Phase 5 personalization product. The `mycel-graph-care` skill we ship here is core product, not personalization; the broader Phase 5 skill that teaches Claude *how to use* the full Mycelium query surface is still future work.
- Anthropic API / OpenAI / Gemini Synthesizer impls. Phase 5 territory; the trait is ready for them.

## Architecture

Three independent changes, each small.

### 1. Cold index: drop synthesis, embed signature + body slice

`mycel index` and the daemon's incremental path stop calling the Synthesizer. The pipeline becomes:

```
parse (tree-sitter)
  → refine (multilspy)
  → upsert Symbol/Edge nodes in FalkorDB
  → embed `signature + body[:60 lines, capped at ~1500 tokens]`
  → write embedding + content_hash
```

The embed content change matters: pure-signature embeddings fail on bad names (`process`, `handle`, `run` — most names). Including 60 lines of body gives the embedder enough surface to distinguish symbols by what they *do*, even when the name is meaningless. 60 lines mirrors the existing synthesizer's `BODY_LINE_CAP` and fits EmbeddingGemma's 2048-token context with margin.

Indexing this workspace should drop from 30 minutes to ~2–3 minutes (embedding-bound, parallelizable). A 50k-symbol TypeScript codebase becomes ~30 minutes instead of multiple days.

`MYCEL_SYNTHESIZER=off` becomes the default behavior, not an escape hatch. The env var is retained for backward compatibility but is now redundant for the indexing path.

### 2. `mycel synthesize` becomes opt-in bulk only

The command, the prompt builder, the atomic write — all retained. Behavior change: it no longer runs as a tail step of `mycel index`, and the `--no-descriptions` flag is removed (no longer meaningful). Users who want the eager-synth experience run `mycel synthesize` explicitly. Provider stays swappable via existing config.

This is the fallback path for users not running Claude Code (CI environments, scripted indexing, non-Claude workflows).

### 3. New write path: `mycel set-description` + the `mycel-graph-care` skill

A new CLI command:

```sh
mycel set-description --qname <qualified-name> --description <text>
```

It looks up the Symbol by qualified name, runs the configured Embedder on the description text, and calls the existing `set_symbol_description_and_embedding` graph method (atomic). It also sets `description_source_hash` to the Symbol's current `content_hash` so staleness can be detected later.

The `mycel-graph-care` skill ships in `skills/mycel-graph-care/` at the repo root. `mycel install` (existing command) symlinks it into `~/.claude/plugins/<plugin>/skills/` alongside the rest of the Mycelium plugin payload. Skill content (high level — final wording lives in the skill file):

- **Trigger** (conservative): after Claude has read a function's body *and* reasoned about its behavior to answer the user's question, write a 1–3 sentence behavioral description and call `mycel set-description`.
- **Anti-triggers**: don't synthesize while skimming for bugs, don't synthesize trivial getters/setters/constants, don't synthesize generated code or test fixtures, don't synthesize `mod foo;` declarations (the hallucination problem CLAUDE.md already documents).
- **Format**: 1–3 sentences. Behavioral, not syntactic. Don't restate the signature. Don't start with "This function…". No code blocks.
- **Conflict handling**: if a description already exists, only overwrite if Claude's understanding is materially different from what's there (skill includes a check via `mycel describe <qname>` first).

The skill is a *behavior policy* — Claude interprets and applies it. We are explicitly not building enforcement; we accept that different sessions will interpret "reasoned about its behavior" with some variance and iterate on the wording.

### 4. Schema: `description_source_hash`

Add one field to the Symbol node:

- `description_source_hash: String?` — the `content_hash` of the Symbol at the time its description was written. NULL for symbols without a description.

Daemon's incremental indexing already computes `content_hash` per Symbol. Add one check on update: if a Symbol's `description_source_hash` is non-NULL and `content_hash != description_source_hash`, clear the description and revert the embedding to the cold-index variant (signature + body slice). The Symbol's behavioral description was written against a function body that no longer exists; better to fall back to a fresh signature+body embedding than serve a stale description that misleads `find`.

This requires a new `mycel-graph` method, e.g., `clear_symbol_description_and_reembed(qname, fallback_text, fallback_vec)`. Migration runs idempotently (existing graphs without the field get NULL on first read).

## Data flows

### Cold index (fresh repo, no existing graph)

1. `mycel index` runs the pipeline above for every file.
2. All Symbols land with signature+body embeddings, NULL descriptions, NULL `description_source_hash`.
3. `mycel find` works immediately on signature+body vectors. Quality is "Cursor-baseline" — solid for most queries, weaker on very-bad-named symbols and behavioral queries.

### Workload-driven synthesis (Claude in active session)

1. User asks Claude to understand or modify some part of the codebase.
2. Claude reads files, reasons about functions to answer the user.
3. Per the `mycel-graph-care` skill, after reasoning about a function, Claude calls `mycel set-description --qname X --description "…"`.
4. CLI embeds the description, atomically writes description + embedding + `description_source_hash`.
5. Future `mycel find` queries that touch this region cluster on behavior, not signature shape.

Token economics: the function body is already in Claude's context for the user's actual task. Emitting a 100-token description is a side-effect, not a separate retrieval. Aggregate cost is bounded by user activity, not codebase size.

### Stale invalidation (file edit hits a described symbol)

1. Daemon's notify watcher fires on file change.
2. Incremental pipeline re-extracts Symbols, computes new `content_hash`.
3. For each updated Symbol with `description_source_hash != content_hash`, clear description and re-embed on signature+body.
4. Next time Claude touches the function, the skill's trigger fires again and a fresh description is written.

### Bulk synthesis fallback (no Claude Code, or one-time backfill)

1. User runs `mycel synthesize [--filter <pattern>] [--force]`.
2. Existing pipeline runs unchanged: prompt builder, configured Synthesizer (Ollama by default), atomic write.
3. `description_source_hash` is set from the current `content_hash` on each write.

## Migration

Existing graphs from the 2026-05-04 Phase 2 ship already have descriptions and description-embeddings. They are kept as-is — those descriptions were generated against valid bodies, and the new `description_source_hash` field defaults to NULL on read.

To bring existing descriptions under the staleness regime, users can run `mycel synthesize --refresh-hashes-only` (a one-shot maintenance flag that walks the graph and sets `description_source_hash = content_hash` for any Symbol with a description but NULL hash). This is a small migration helper, not a re-synthesis.

Users who want to abandon eager-synth descriptions and start fresh can run `mycel index --force-cold-rebuild`, which clears all descriptions and reverts every Symbol to signature+body embedding. They can then re-fill via Claude over time.

## Testing

Unit tests:
- `set-description` CLI: rejects unknown qname, validates that the embedder is called, validates that `description_source_hash` is written.
- Staleness detection: a Symbol whose body changes triggers description clear and re-embed on next incremental pass.
- Cold-index embed content: assert the embed input is `signature + body slice` (not pure signature) and respects the 1500-token cap.

Integration tests (require ephemeral FalkorDB on `redis://127.0.0.1:16379`):
- Full cold-index → `mycel set-description` → `mycel find` cycle: a behaviorally-described symbol scores higher on a behavioral query than its signature-embedded peers.
- Edit-and-stale: index a file, write a description, edit the function body substantially, run incremental, verify description was cleared.
- Migration: load a 2026-05-04-shape graph (descriptions present, hash NULL), run `--refresh-hashes-only`, verify hashes populated and descriptions untouched.

Skill verification (manual, can't be unit-tested):
- Run a session where Claude is asked to debug a bug in a small function. Verify Claude writes a description after reasoning about it, doesn't write one for functions it merely glanced at, and the description is behavioral rather than syntactic.

## Error handling

The existing Phase 2 principle holds: per-symbol synthesis failures are logged at WARN and do not abort. Same for `set-description` writes — a failed embed (Ollama down, network hiccup) returns a non-zero exit code and Claude can retry; the graph stays consistent because the atomic write either lands fully or not at all.

The `set-description` CLI does NOT silently swallow embedder errors. If embedding fails, the command fails, Claude sees the failure, and the description is not written. We have a documented anti-pattern in CLAUDE.md against silent error catching; this command honors it.

## Non-goals

- We are not building enforcement that forces Claude to write descriptions. The skill is a policy; interpretation variance is accepted.
- We are not building a description-quality metric or feedback loop in this revision. If descriptions turn out poor in practice, we iterate on the skill wording, not on infrastructure.
- We are not migrating existing graphs automatically. Users opt in to the maintenance flag if they want stale-detection on legacy descriptions.
- We are not adding cloud Synthesizer impls (Anthropic, OpenAI). Phase 5.

## Open questions (deferred, not blocking)

- Should `mycel set-description` accept a batch of (qname, description) pairs? Useful if the skill ever evolves toward "summarize this whole module's public API in one go." Trivial to add later; not building it now.
- When `mycel find` returns mixed-state results (some described, some not), should ranking weight them differently? Phase 3 reranker territory. For now, single HNSW index, accept the bias, observe whether it's a real problem.
- Should the skill suggest synthesis for symbols Claude *encounters but doesn't read deeply* (e.g., as a callee in graph expansion)? Probably no — too aggressive, drifts toward the "synthesize everything" failure mode we're explicitly leaving behind. Open if usage shows otherwise.

## Phase plan delta

Phase 2 (revised) — workload-driven synthesis: this design.
Phase 3 — reranker, full Tier 4 (vector → graph expand → rerank), reranker eval harness, query-time on-demand synthesis for low-confidence hits.
Phase 4 — git-derived edges, Tier 3 queries.
Phase 5 — broader SKILL.md teaching all of Mycelium's query surface, cloud Synthesizer impls, token measurement, personalization.

The `mycel-graph-care` skill we ship here is the *first* skill in Mycelium's plugin payload, narrowly scoped to "tend the graph as you work." Phase 5's broader skill builds on top of it.
