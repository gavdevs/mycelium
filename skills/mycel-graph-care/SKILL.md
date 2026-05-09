---
name: mycel-graph-care
description: Use after reading and reasoning about a function's behavior to answer the user's question — write a 1-3 sentence behavioral description back to the Mycelium graph so future searches cluster on behavior. Conservative trigger; do not synthesize while skimming, do not synthesize trivial code.
---

# Tend the Mycelium graph as you work

Mycelium watches this repo and indexes every Symbol with a fast embedding
of its signature + body. That gets you Cursor-grade semantic search for
free. The behavioral-search advantage — clustering on what code DOES rather
than what it's NAMED — only shows up once Symbols have synthesized
descriptions. Cold indexing doesn't write descriptions to keep indexing
fast. You write them as a side-effect of normal work.

## When to write a description (conservative trigger)

Write only when ALL of these are true:

- You read the symbol's body via `Read` or via `mycel` output
- You used your understanding to make a decision, an explanation, or a code change
- The symbol is non-trivial: more than ~10 lines, or genuinely complex single-line logic

## When NOT to write

- You skimmed a file looking for an unrelated bug — don't synthesize what you didn't think about
- The symbol is a getter, setter, constant, simple struct field, or generated code
- The symbol is a `mod foo;` declaration (Rust module decls hallucinate badly)
- The symbol is in `tests/`, `examples/`, or a `*_test.*` file
- A description already exists AND your understanding matches it (call `mycel describe` first)

## How to write

1. Get the canonical qualified name:

   ```sh
   target/release/mycel --repo . definers <name> --json
   ```

   Take the `qualified_name` field from the result.

2. Check current state:

   ```sh
   target/release/mycel --repo . describe <qname>
   ```

   If it prints something and your understanding matches, skip. If it differs
   materially, overwrite. If it prints `(none)`, write a fresh description.

3. Write the description:

   ```sh
   target/release/mycel --repo . set-description \
     --qname <qname> \
     --description "<1-3 sentences>"
   ```

## Description format

- 1-3 sentences. Behavioral, not syntactic.
- Don't restate the signature ("takes X, returns Y") — the signature is already there.
- Don't start with "This function..." — get to the verb.
- Focus on what the symbol does, why it exists, what role it plays.
- No code blocks, no markdown.

## Examples

Good:

> Atomically writes a Symbol's synthesized description and the embedding
> derived from it, stamping the description's source body hash so the
> daemon can detect staleness on later edits.

Good:

> Picks the default synthesizer model from the configured tier — gemma4:e2b
> on minimal, gemma4:e4b on balanced, qwen3.6:35b-a3b on max. Auto-detects
> tier from /proc/meminfo when not pinned.

Bad ("restates the signature"):

> Takes a qname, description, and embedding, writes them to the graph, returns Result<()>.

Bad ("vague"):

> Handles description writes for the indexer.

## What to do if a command fails

- `mycel definers <name>` returns nothing — the symbol may not be indexed
  (file just edited and daemon hasn't caught up); wait 5 seconds and retry,
  or fall back to grepping the source.
- `mycel set-description` exits non-zero with "no Symbol with qualified_name" —
  the qname is wrong. Re-fetch it from `mycel definers <name> --json`.
- `mycel set-description` exits non-zero with an embedder error — Ollama
  likely isn't running. Confirm with `curl http://localhost:11434/api/tags`
  and retry. Don't ask the user to restart Ollama unless they explicitly
  asked you to handle errors.
