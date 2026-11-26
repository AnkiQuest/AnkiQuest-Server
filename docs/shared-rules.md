# Shared rules

`crates/rules` (`ankiquest-rules`) holds rules the server and both clients compute, written once
in Rust, the way Anki shares its Rust core with AnkiDroid through Anki-Android-Backend. Until now
each rule existed three times (Rust on the server, Kotlin on Android, Python in the add-on), kept in
line only by the cases in `tests/fixtures/*.json`. The fixtures stay: they test every caller of the
crate, the bindings included.

```
crates/rules (Rust, in AnkiQuest-Server)
  ├─ server: uses it directly
  ├─ UniFFI → Kotlin + .so per ABI → Android fork and Lite (AAR)
  └─ UniFFI → Python + native lib per OS → desktop add-on
```

## What is shared

The pilot rule is today's day verdict (`tests/fixtures/day_verdicts.json`).

- `today_verdict(server: ServerDay, pending: Vec<PendingReview>, now_ms: i64) -> DayVerdict`:
  what the server's profile said about today, with the reviews the client has not uploaded. A
  `PendingReview` without `time_ms` is an observed answer from AnkiQuest Lite.
  `DayVerdict` is `Ended` (the answer is about a day that has ended) or `Today { verdict }`.
  `TodayVerdict` has the fixture's fields: `met`, `streak_state`, `streak`, `reviews`, `observed`,
  `time_ms`, `minutes`, `streak_minutes`, `unsent`, `label`.
- `StreakState` (`studied`, `unverified`, `pending`, `protected`) and `DayLabel` (`studied`,
  `unverified`, `protected`, `short`, `pending`, `unknown`).
- `meets_minimum(reviews, time_ms, needed_ms)`: whether a day counts toward the streak. Rust only;
  the server uses it for every day it judges.

The optimize reminder (`tests/fixtures/optimize_reminder.json`): when to remind the player to run
Anki's Optimize All Presets. The server only validates its state, the synced setting
`optimize_reminder`; the clients compute the rule. Until they use the bindings, each keeps its own
copy (`AnkiquestOptimizeReminder.kt`, `addon/optimize_reminder.py`), tested against the same cases.

- `preset_scheduler(flags: PresetFlags) -> PresetScheduler` (`sm2`, `fsrs`, `rwkv_curve`,
  `rwkv_instant`): a preset's scheduler from `fsrs`, `rwkvReviewEnabled` and
  `rwkvReviewInstantOrderEnabled`.
- `answers_start_ms(today, day_cutoff, last_optimized_day)` and `is_answer(revlog_type, ease)`:
  which revlog entries count as answers since the last optimization.
- `optimize_verdict(inputs: OptimizeInputs, state: OptimizeState) -> OptimizeVerdict`: `Off`,
  `NoFsrs`, `Snoozed`, `Dismissed`, `TooSoon`, `TooFewReviews` or `Remind { since, wording }`.
- `optimize_next(state, event, today, last_optimized_day) -> OptimizeState`: the state after
  snooze, dismiss, turn off or turn on. `OptimizeState` is `On`, `Off`, `Snoozed { until_day }` or
  `Dismissed { optimized_day }`, in the setting's JSON with the `serde` feature.

The crate is pure: no IO, no async, no server code. Its features:

- `ffi`: the UniFFI exports (proc macros, no UDL).
- `serde`: the server's JSON names.
- `bindgen`: the `uniffi-bindgen` binary.

## How clients get it

The `Shared rules` workflow (`.github/workflows/rules.yml`) builds, on every pull request and
`master` push:

| Artifact | Contents |
| --- | --- |
| `ankiquest-rules-bindings` | `kotlin/org/ankiquest/rules/ankiquest_rules.kt`, `python/ankiquest_rules.py` |
| `ankiquest-rules-android` | `jniLibs/<abi>/libankiquest_rules.so` for arm64-v8a, armeabi-v7a, x86 and x86_64 (API 24, NDK r28c, 16 KB pages), the Kotlin sources, `SHA256SUMS` |
| `ankiquest-rules-<os>-<arch>` | the library for linux-x86_64, windows-x86_64, macos-arm64 or macos-x86_64, `ankiquest_rules.py`, `SHA256SUMS` |

The Kotlin bindings need JNA (`net.java.dev.jna:jna:<version>@aar` on Android). The Python module
loads the library next to it with `ctypes`; it is not a compiled extension, so one library works
with every Python version, Anki's bundled one included. CI runs it on Python 3.9 (Anki 24.11) and
3.13 on all three operating systems, and the Kotlin on the JVM, against the fixture cases.

There is no AAR yet: it needs the Android Gradle plugin and SDK here, and the clients compile the
Kotlin sources with their own Kotlin version anyway. The jniLibs and sources go into the client's
`src/main` directories as they are.

## Reproducible builds

A commit always builds the same bytes, so a client can check what it ships against the server's
CI:

- `crates/rules/rust-toolchain.toml` pins the toolchain for builds run in `crates/rules`.
- `--locked` builds with the workspace `Cargo.lock`.
- The `bindings` profile (root `Cargo.toml`): `opt-level = "s"`, LTO, one codegen unit, stripped,
  `panic = "unwind"` so UniFFI turns a panic into an exception instead of ending the app.
- No build id on Linux and Android (`-Wl,--build-id=none`), `/Brepro` on Windows; ld64 already
  derives the macOS UUID from the contents. `--remap-path-prefix` removes the checkout and Cargo
  paths.
- CI builds every library twice from a clean target directory and fails unless the hashes match.

Bindings are generated from an unstripped debug build, since stripping removes the UniFFI metadata
symbols; they do not depend on the target.

## Phase 2

Each client replaces its day-verdict code with the bindings and keeps running
`day_verdicts.json` against them:

- **Android fork**: add the jniLibs and Kotlin sources, JNA, and swap the Kotlin rule for
  `todayVerdict`. Measure the APK size per ABI before and after.
- **Lite `:ankiquest-core`** (AnkiQuest-Android#421): the same, inside the module.
- **Desktop add-on**: ship the four libraries and `ankiquest_rules.py` in the package, load the one
  for the running OS, and swap the Python rule. Measure on Anki 24.11 and the newest Anki on Linux,
  Windows and macOS (arm64 and x86_64), and the AnkiWeb package size before and after.

Each client already pins a server commit (`ankiquest-server.sha` on Android). The client's CI
downloads the artifacts of the `Shared rules` run for that commit on `master` and checks them
against `SHA256SUMS`; building from source with cargo-ndk in every client CI would repeat the NDK,
toolchain and four-OS matrix for the same bytes. Artifacts expire after 90 days, so a pinned commit
needs a release (a tag with the artifacts attached) before then; tagging releases is part of
phase 2. Building from source stays possible, and reproducible, for anyone who wants to check.

Further rules (heatmap, streak moods, card labels, deck names) move into the crate one at a time
after the pilot, each with its fixture.
