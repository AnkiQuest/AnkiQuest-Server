# Keeping released Android clients compatible

Both clients test against this repository's `master`: their CI fetches
`docs/api/examples/*.json` and `tests/fixtures/*.json` when the tests run,
and nothing pins or copies them. A server change that breaks a client's
tests is fixed in the same change, or the client is updated right after.

The `android-contract` CI job guards the examples Android reads. It compares
each of them with the same example at the base commit (the pull request's,
the merge group's or the push's base, from `xtask event-base`) and runs a
small Rust guard (`crates/xtask`). It reads Android's catalog from a sparse
public checkout of `AnkiQuest/AnkiQuest-Android@master`, without a personal
access token, and does not run Android code or Gradle.

Android's `ankiquest-api/names.json` (in its test resources) lists response
names in arrays called `consumed`, `boards`, `challenges`, `unused` and
`not_yet`. The first three count as consumed; the last two do not. An
example Android doesn't list counts as `not_yet` there.

The guard fails if a consumed example disappears, a JSON key path is
removed, or a type seen at that path is lost. Array elements share a `[*]`
path, preserving the types and keys represented anywhere in the base
commit's array. Booleans are distinct from numbers; integer and decimal
numbers are the same JSON type. Values, new keys, new array variants and
new examples are allowed. An empty array cannot replace one that exercises
member fields without losing that coverage.

This is a guard for recorded response shapes, not a complete API schema or
a replacement for the clients' contract tests, which run against `master`.
Preserve behavior for released apps too: a compatible recording cannot
prove that every response or optional-field combination is safe.

Run the guard locally against another commit's examples:

```sh
cargo test -p ankiquest-xtask contract
mkdir -p /tmp/base && git archive origin/master docs/api/examples | tar -x -C /tmp/base
cargo run -p ankiquest-xtask -- android-contract \
  --android-catalog ../AnkiQuest-Android/AnkiDroid/src/test/resources/ankiquest-api/names.json \
  --baseline /tmp/base/docs/api/examples
```

To retire a consumed example, first land the Android change that stops
reading it and updates the catalog, then wait until released apps no longer
need it before removing the server behavior and recording.

**android-contract** should be a required check on `master`, with pull
requests required and the rules applied to administrators too. A workflow
file does not change branch protection.
