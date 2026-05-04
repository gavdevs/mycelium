# ts-sample

A small original TypeScript fixture used by `tests/e2e_dogfood.sh` to verify
Mycelium's TS indexing end-to-end. Hand-written for this purpose; not derived
from any external project.

## Layout

- `src/types.ts` — domain types (User, Session)
- `src/auth.ts` — type-checking utilities
- `src/cache.ts` — generic cache with eviction
- `src/store.ts` — in-memory store using cache
- `src/index.ts` — public surface
- `src/utils.ts` — small helpers
