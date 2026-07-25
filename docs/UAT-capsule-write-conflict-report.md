# UAT — Capsule declared-write conflict guard

Live black-box verification of the `/load` write-key conflict check added in response to a
design review comment on the capsules feature: unloading a capsule frees its system-prompt
text but not any shared session state it may have written, so two capsules claiming the same
state key can produce hard-to-trace "ghost" behavior. Since v1 has no session-state store to
retrofit, the fix implemented is the load-time half of the guard from
`docs/superpowers/specs/2026-07-23-capsules-design.md`'s "Design constraint" section: a capsule
declares the session keys it intends to write via an optional `writes:` frontmatter field, and
`/load` rejects activating a capsule whose declared keys overlap an already-active capsule's.

## Method

Built the release binary (`cargo build --release -p quecto-agent`) and drove the real REPL —
no mocked model, `--provider openai --model none` since the scenario only exercises
`/capsules`/`/load`/`/unload`, which never call the model — in a live cmux terminal pane against
three fixture capsules under `/tmp/quecto-uat-writes/.quecto/capsules/`:

| Capsule | `writes:` |
|---|---|
| `alpha` | `theme` |
| `beta` | `theme, layout` |
| `gamma` | `layout` |

## Scenarios and results

1. **`/capsules`** before anything is loaded — all three discovered and listed. ✅
2. **`/load alpha`** → `loaded alpha`. ✅
3. **`/load beta`** while `alpha` is active → rejected:
   `cannot load beta: write key "theme" is already claimed by active capsule alpha`. `beta` does
   not appear active in the following `/capsules`. ✅ (matches the exact message format specified
   in the design doc's error-handling table)
4. **`/load gamma`** while `alpha` is active → `loaded gamma` (disjoint key `layout`, no
   conflict). ✅
5. **`/unload alpha`**, then **`/load beta`** (still blocked, `gamma` holds `layout`) →
   `cannot load beta: write key "layout" is already claimed by active capsule gamma`. This
   confirms the conflict check walks *all* of a candidate's declared keys against *all* active
   capsules, not just the first key or the first active capsule. ✅
6. **Fresh REPL restart**, `/unload gamma` → `gamma is not loaded` (active-capsule set is
   session-scoped and does not persist across restarts, matching documented v1 behavior), then
   `/load beta` succeeds cleanly with nothing else active. ✅

## Outcome

6/6 scenarios passed. No blocking defects. Automated coverage added alongside this UAT:
`load_rejects_a_capsule_whose_write_key_is_already_claimed`,
`load_allows_capsules_with_disjoint_write_keys`, `writes_parses_comma_separated_keys`,
`writes_defaults_to_empty_when_missing` in `quecto-agent/src/capsule.rs`. Full workspace test
suite green (`cargo test`), clippy clean of new warnings.

**Known scope boundary** (documented, not a defect): this guard only covers keys a capsule
*declares*. No capsule capability writes actual shared session state yet — v1's only shared
state is the system prompt, which is fully recomputed on every load/unload — so there is
nothing yet to enforce an *undeclared* write against, and `/unload`'s scope-clear/ownership
contract (point 3 in the design doc) remains intentionally undesigned-in-code until a feature
needs real session-state writes.
