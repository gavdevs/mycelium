# Mycelium — Workload-Driven Synthesis (Phase 2 policy revision)

**Status**: Approved design as of 2026-05-05. Revises the Phase 2 policy shipped 2026-05-04. Phase 2's *infrastructure* (Synthesizer trait, Ollama impl, prompt builder, atomic description+embedding write) survives unchanged. What changes is *when* synthesis runs and *who* runs it.

## Why we changed direction

Phase 2 shipped behavioral description synthesis on 2026-05-04: for every Symbol in the graph, build a prompt from signature + body slice + 1-hop neighbors, call a local Ollama Synthesizer, persist a description, re-embed on it. The retrieval thesis was sound — descriptions cluster by behavior, not by surface syntax — but the *trigger* was wrong.

Concrete dogfood result: indexing this Rust workspace (a few hundred symbols) took **30 minutes**. Linear extrapolation puts a typical work-scale TypeScript codebase (~50k symbols) at multiple days. The per-symbol Ollama round-trip is the bottleneck, and it doesn't matter how fast the model is — the topology of "one LLM call per symbol, every symbol" is wrong. Even if we swapped local Ollama for a paid cloud Synthesizer running thousands of concurrent requests, we'd be paying real money to synthesize descriptions for symbols nobody ever queries.

Cursor solves the same indexing problem by *not* synthesizing at index time at all: chunk files, embed chunks, do understanding at query time. Their cold-index pass is embedding-bound, which is roughly two orders of magnitude faster than what we shipped.

This design adopts Cursor's cold-index posture **and** preserves Phase 2's behavioral-search bet by moving synthesis off the indexing hot path and onto a workload-driven trigger: Claude Code itself, working in the codebase as part of normal user tasks, writes descriptions for the symbols it has actually read and reasoned about. The function body is already loaded in Claude's context for the user's real task — emitting a 1–3 sentence description is a side-effect, not a separate retrieval. Marginal token cost is near-zero, and tokens are billed against whatever subscription/API the user is already paying for.

The upshot:
- Mycelium's indexer becomes Cursor-fast for cold builds (~2–3 minutes for this workspace, ~30 minutes for a 50k-symbol work codebase, instead of multiple days).
- The behavioral-search advantage emerges over time, scoped to the regions of the codebase Claude actually touches — which is exactly where retrieval quality matters most.
- This *is* the Mycelium thesis distilled: "make Claude smarter on this codebase as it works in it." Workload-driven synthesis is that mechanism.

## Scope

**In scope:**
- Replace eager-at-index synthesis with a Cursor-style cold index that embeds `signature + body slice`. Update `mycel index` and the daemon's incremental pipeline.
- Demote `mycel synthesize` to an opt-in bulk command. `mycel index` no longer triggers it. The `--no-descriptions` flag on `mycel index` is removed (no longer meaningful).
- New CLI command **`mycel set-description --qname <qname> --description <text>`** — runs the embedder on the description text and atomically writes description + embedding + `description_source_hash` to the named Symbol.
- New CLI command **`mycel describe <qname>`** — reads and prints the current description (and `description_source_hash` status) for a Symbol. Used by the skill to decide whether to overwrite.
- New CLI command **`mycel skill install`** — one-line helper that symlinks `<repo>/skills/mycel-graph-care/` into `~/.claude/skills/mycel-graph-care/`. Idempotent. No plugin manifest scaffolding in this revision; the skill is a single SKILL.md file.
- New `mycel synthesize --refresh-hashes-only` flag — one-shot maintenance pass that walks the graph and sets `description_source_hash = body_hash` for every Symbol that already has a description but a NULL hash. Used to bring 2026-05-04-shaped graphs under the new staleness regime without re-synthesizing anything.
- New `mycel index --force-cold-rebuild` flag — clears all descriptions and reverts every Symbol to signature+body embedding. Escape hatch for users abandoning eager-synth descriptions.
- Schema additions on the Symbol node: `body_hash: String` (always populated), `description_source_hash: String?` (NULL until a description is written, then mirrors `body_hash` at write time).
- Daemon staleness handling: when an incremental pass updates a Symbol whose `description_source_hash` is non-NULL and `body_hash != description_source_hash`, clear the description and revert that Symbol's embedding to the cold-index variant.
- Ship a Mycelium skill `mycel-graph-care` at `<repo>/skills/mycel-graph-care/SKILL.md`. Skill content drafted in §"Skill body draft" below.
- Update `CLAUDE.md` and `DESIGN.md` to reflect the new policy.

**Explicitly out of scope:**
- Changes to the Synthesizer trait surface or the existing Ollama / future cloud impls. Bulk synthesis remains available unchanged.
- Reranker, query-time on-demand synthesis on low-confidence `find` hits, BM25 hybrid retrieval — Phase 3 work.
- Cross-file CALLS / IMPLEMENTS / TYPED_BY edge resolution — separately tracked Phase 2/3 work.
- A broader Mycelium SKILL.md teaching all of Mycelium's query surface (`find`, `definers`, etc.) — Phase 5 product, separate brainstorm. The `mycel-graph-care` skill we ship here is narrowly scoped to "tend the graph as you work" and is the *first* skill in Mycelium's payload.
- Anthropic API / OpenAI / Gemini Synthesizer impls — Phase 5.
- Plugin manifest (`plugin.json`) scaffolding, Claude Code plugin marketplace listing — Phase 5.
- Multi-session description coordination beyond last-writer-wins — see "Concurrency and ordering" below.

## Architecture

Four independent changes, each small.

### 1. Cold-index pipeline change

`mycel index` and the daemon's incremental path stop calling the Synthesizer. The pipeline becomes:

```
parse (tree-sitter)
  → refine (multilspy)
  → upsert Symbol/Edge nodes in FalkorDB
  → compute body_hash = blake3(signature || "\n" || body[:60 lines, capped at ~1500 tokens])
  → embed the same content used for body_hash
  → write embedding + body_hash atomically with the Symbol
```

The embed content change matters: pure-signature embeddings fail badly on bad names (`process`, `handle`, `run` — most names in any large codebase). Including 60 lines of body text gives the embedder enough surface to distinguish symbols by what they *do* even when names are useless. The 60-line cap is the same constant as the existing synthesizer's `BODY_LINE_CAP` (in `crates/mycel-index/src/synthesize.rs`); we reuse the value rather than retune it.

`body_hash` and the embedding both derive from the same input string, so they're always consistent: if `body_hash` changed, the embedding has been updated to match.

`MYCEL_SYNTHESIZER=off` becomes the default behavior, not an escape hatch. The env var stays for backward compatibility but is now redundant for the indexing path.

### 2. `mycel synthesize` becomes opt-in bulk only

The command, the prompt builder, and the atomic write are retained unchanged. Behavior change: it no longer runs as a tail step of `mycel index`, the `--no-descriptions` flag is removed, and `--refresh-hashes-only` is added.

When `mycel synthesize` writes a description, it now also writes `description_source_hash = body_hash` for that Symbol.

This is the fallback path for users not running Claude Code (CI environments, scripted indexing, non-Claude workflows).

### 3. Write path: `mycel set-description` + `mycel describe`

**`mycel set-description --qname <qname> --description <text>`** does:

1. Look up the Symbol by `qname`. If not found, exit non-zero with a clear error (Claude can recover by calling `mycel definers <name>` to get a canonical qname).
2. Run the configured Embedder on `<text>`.
3. Call the existing `set_symbol_description_and_embedding` graph method (atomic: description + embedding land together or not at all).
4. Set `description_source_hash = body_hash` on the same write.

**`mycel describe <qname>`** does:

1. Look up the Symbol. If not found, exit non-zero.
2. Print the current description, or the literal string `"(none)"` if NULL.
3. With `--json`, also include `description_source_hash` and current `body_hash` so callers can detect staleness without fetching the body. Used by the skill's "should I overwrite?" check.

Both commands fail loudly on embedder/graph errors. We do not silently swallow — the existing CLAUDE.md anti-pattern against silent error handling applies.

**Canonical qname workflow.** The `qname` Claude passes to `set-description` must match the qname stored in the graph exactly. The skill teaches Claude to read qnames from `mycel definers <name> --json` (which Tier-1 already produces reliably) rather than constructing them by hand.

### 4. Skill: `mycel-graph-care`

Ships at `<repo>/skills/mycel-graph-care/SKILL.md`. Users install it with `mycel skill install`, which symlinks `<repo>/skills/mycel-graph-care/` into `~/.claude/skills/mycel-graph-care/`. The install is idempotent and the symlink keeps the file under version control in this repo (changes to the skill ride along with `git pull`).

The skill is a *behavior policy* — Claude interprets and applies it. We are explicitly not building enforcement; we accept that different sessions will interpret the trigger conditions with some variance and iterate on wording.

#### Skill body draft (final wording lives in the skill file; this is the planning artifact)

```markdown
---
name: mycel-graph-care
description: Use when you've read a function's body and reasoned about its behavior to answer the user's question — write a 1-3 sentence behavioral description back to the Mycelium graph so future searches cluster on behavior. Conservative trigger; do not synthesize while skimming.
---

# Tend the Mycelium graph as you work

When you've read a function (or method, struct, type) AND reasoned about its
behavior to answer the user's actual question, write a short behavioral
description back to Mycelium. Future `mycel find` queries against that region
will cluster on what the code does instead of what it's named.

## When to write a description (conservative trigger)

Write only when ALL are true:
- You read the symbol's body via Read or via mycel output.
- You used your understanding to make a decision, explanation, or change.
- The symbol is non-trivial (>10 lines, or genuinely complex single-line logic).

## When NOT to write

- You skimmed the file looking for an unrelated bug.
- The symbol is a getter, setter, constant, or generated code.
- The symbol is a `mod foo;` declaration (these hallucinate badly).
- The symbol is in `tests/` or `examples/` or a `*_test.*` file.
- A description already exists and your understanding matches it.

## How to write

1. Get the canonical qname:
   `mycel --repo . definers <name> --json` → take the `qualified_name` field.
2. Check current state:
   `mycel --repo . describe <qname> --json` → if `description` is non-null
   AND your understanding matches, skip. If it differs materially, overwrite.
3. Write: `mycel --repo . set-description --qname <qname> --description "<1-3 sentences>"`

## Description format

- 1-3 sentences. Behavioral, not syntactic.
- Don't restate the signature ("takes X, returns Y").
- Don't start with "This function...".
- Focus on what the symbol does, why it exists, what role it plays.
- No code blocks. No markdown.
```

The skill ships as a single SKILL.md file. No supporting scripts or assets in this revision.

### 5. Schema delta

Two new fields on the Symbol node:

| Field | Type | Populated when | Meaning |
|-------|------|----------------|---------|
| `body_hash` | `String` (blake3 hex) | Always, set during cold index and on every incremental update | Hash of the exact `signature + body slice` string used for embedding. Used to detect when a Symbol's *embedding source* has changed. |
| `description_source_hash` | `String?` | NULL until a description is written; set to the `body_hash` value at description write time | Used to detect when a description has gone stale because the body changed after it was written. |

Schema migration is forward-only and idempotent: existing Symbols read NULL for both fields on first access and get `body_hash` populated on the next incremental pass (or via `mycel index --force-cold-rebuild`).

A new `mycel-graph` method, `clear_symbol_description_and_reembed(qname, fallback_text, fallback_vec)`, performs the staleness fallback atomically.

## Concurrency and ordering

Four interleavings to think about. Last-writer-wins on `description_source_hash` makes all of them self-consistent.

| Sequence | Resulting state | Self-consistent? |
|----------|-----------------|------------------|
| Claude reads function → writes description → user edits function → daemon clears description | NULL description, fresh signature+body embedding, `body_hash` updated | ✅ — staleness was correctly detected |
| User edits function → daemon updates `body_hash` → Claude (in same session, hasn't reread file) writes description from stale memory | Description stored against new `body_hash`; description content is wrong but the schema is consistent | ⚠ — content stale, schema consistent. Mitigated by the skill instructing Claude to base descriptions only on freshly-read code. Detection of this case is out of scope. |
| Two parallel Claude sessions write descriptions for the same qname | Last write wins; both writes are atomic at the Cypher boundary | ✅ |
| `set-description` lands while daemon is mid-flight on incremental update of the same file | Both writes are atomic at the Cypher boundary; whichever lands second is the final state | ✅ |

The third row's "two parallel Claude sessions" case is rare in practice (Mycelium is single-user, and a user rarely drives two sessions against the same file at once), but the atomic-write guarantee at the Cypher boundary handles it for free.

The second row is the only genuinely ambiguous case. We accept it as a small known-bad mode; the skill mitigates by encouraging "just-read" semantics.

## Data flows

### Cold index (fresh repo)

1. `mycel index` runs the new pipeline for every file.
2. All Symbols land with `signature + body slice` embeddings, populated `body_hash`, NULL `synthesized_description`, NULL `description_source_hash`.
3. `mycel find` works immediately on signature+body vectors. Quality baseline = "Cursor-equivalent" — solid for most queries, weaker on bad-named symbols and purely-behavioral queries.

### Workload-driven synthesis (Claude in active session)

1. User asks Claude to understand or modify some part of the codebase.
2. Claude reads files, reasons about functions to answer the user.
3. Per `mycel-graph-care` skill rules, Claude calls `mycel definers <name> --json` to get the qname, optionally `mycel describe <qname>` to check current state, then `mycel set-description ...` if appropriate.
4. CLI embeds the description, atomically writes description + embedding + `description_source_hash`.
5. Future `mycel find` queries that touch this region cluster on behavior.

### Stale invalidation (file edit hits a described symbol)

1. Daemon's notify watcher fires on file change.
2. Incremental pipeline re-extracts Symbols, computes new `body_hash`.
3. For each updated Symbol with non-NULL `description_source_hash` and `body_hash != description_source_hash`, the daemon calls `clear_symbol_description_and_reembed(qname, signature+body, embed(signature+body))`.
4. Next time Claude touches the function and decides it's worth describing, the skill's trigger fires and a fresh description is written.

### Bulk synthesis fallback (no Claude Code, or one-time backfill)

1. User runs `mycel synthesize [--filter <pattern>] [--force]`.
2. Existing pipeline runs unchanged; `description_source_hash` is set from the current `body_hash` on each write.

### Migration (legacy 2026-05-04-shape graphs)

Two routes:
- **Keep existing descriptions, add staleness tracking**: `mycel synthesize --refresh-hashes-only`. Walks the graph, sets `description_source_hash = body_hash` for every Symbol with a description but a NULL hash. Does not call the Synthesizer.
- **Abandon eager-synth and start fresh**: `mycel index --force-cold-rebuild`. Clears all descriptions, reverts every Symbol to signature+body embedding. Claude refills opportunistically over time.

Doing nothing also works: legacy descriptions stay, are never invalidated by edit (because the hash is NULL), and gradually drift from accuracy. Users can opt into the staleness regime later.

## Success criteria

We need lightweight checks that this is actually better than what we shipped, not just faster.

1. **Cold-index speed**: `mycel index .` from a clean graph completes in under 5 minutes on this workspace, on tier `balanced` hardware. (Current ship: 30 minutes.)
2. **Find quality on cold-indexed graph**: a hand-curated set of ~10 representative behavioral queries on this repo (e.g., "atomic write of description and embedding", "tier detection from system memory") returns the right Symbol in the top-5 vector hits at least 70% of the time. Pure-signature embeddings fail this badly; signature+body slices should comfortably clear it.
3. **Description coverage after a week of dogfood use**: at least 30 Symbols on this repo have non-NULL descriptions written by Claude via the skill, with no manual `mycel synthesize` runs.
4. **Staleness correctness**: edit a function whose description was written by Claude, run incremental indexing, verify the description is cleared and the embedding reverted to signature+body.

(1) and (2) are pre-merge gates. (3) is observed post-ship over normal use. (4) is an integration test.

## Testing

**Unit tests:**
- `set-description` CLI: rejects unknown qname; validates that the embedder is called and that `description_source_hash` is written; fails loudly on embedder error.
- `describe` CLI: prints `"(none)"` for NULL description; emits well-formed JSON with `--json`.
- Cold-index embed content: assert that the embed input is `signature + body[:60 lines]` capped at ~1500 tokens (not pure signature, not full body).
- `body_hash` stability: same content → same hash.
- Staleness trigger: a Symbol whose body changes substantially gets its description cleared on next incremental pass.

**Integration tests** (require ephemeral FalkorDB on `redis://127.0.0.1:16379`):
- Cold-index → `mycel set-description` → `mycel find` cycle: a Symbol with a behavioral description scores higher on a behavioral query than its signature-embedded peers.
- Edit-and-stale: index a file, write a description for a function, edit the function body, run incremental indexing, verify description was cleared.
- Migration: load a 2026-05-04-shape graph (descriptions present, hash NULL), run `--refresh-hashes-only`, verify hashes populated and descriptions untouched.
- Concurrency: two `set-description` calls for the same qname, run concurrently — last-writer-wins, no torn writes.

**Skill verification** (manual; cannot be unit-tested): run a session asking Claude to debug or explain a small function. Verify Claude writes a description after reasoning, doesn't write one for trivial code, and the description is behavioral.

## Error handling

- Per-symbol failures during cold index are logged at WARN and do not abort the run (existing Phase 2 principle preserved).
- `set-description` and `describe` fail loudly. No silent swallowing of embedder or graph errors. Failed `set-description` writes leave the graph unchanged (atomic write at the Cypher boundary).
- Staleness invalidation that fails (e.g., embedder unreachable when computing the fallback signature+body embedding) is logged at WARN and the description is *not* cleared — better to serve a stale description than to leave a Symbol with NULL embedding. Next incremental pass retries.

## DESIGN.md updates included with this work

- Phase 2 description rewritten to reflect workload-driven synthesis as the policy. Add a brief italicized callout explaining the post-2026-05-04 redirection so the rationale is visible to future readers.
- Phase 1 description updated to note that `find` is now `signature + body slice` embedded (not pure signature), as of this revision.
- Schema section updated with `body_hash` and `description_source_hash` on Symbol.

## Non-goals

- Enforcement of skill rules. Interpretation variance across sessions is accepted.
- Description-quality metric or feedback loop. If descriptions are bad in practice, iterate on the skill wording, not on infrastructure.
- Automatic migration of existing graphs. Users opt into the maintenance flag.
- Cloud Synthesizer impls. Phase 5.
- A `mycel-aware` general SKILL.md that teaches Claude to navigate Mycelium overall. Phase 5; this revision ships only `mycel-graph-care`.

## Open questions (deferred, not blocking)

- Should `mycel set-description` accept a batch of (qname, description) pairs? Trivial to add later; not building it now.
- Mixed-state `find` ranking (some Symbols described, some not): single HNSW index for now, accept any bias, observe in practice. Phase 3 reranker territory if it becomes a problem.
- Whether to expose a `mycel stats` command summarizing description coverage (% of Symbols described, average description age, staleness rate). Useful for observing whether the workload-driven hypothesis pans out, but not blocking the ship — can be added trivially after.

## Phase plan delta

- **Phase 2 (revised)** — workload-driven synthesis: this design.
- **Phase 3** — reranker, full Tier 4 (vector → graph expand → rerank), reranker eval harness, query-time on-demand synthesis for low-confidence `find` hits.
- **Phase 4** — git-derived edges, Tier 3 queries.
- **Phase 5** — broader SKILL.md teaching all of Mycelium's query surface, plugin payload + marketplace listing, cloud Synthesizer impls, token measurement, personalization.
