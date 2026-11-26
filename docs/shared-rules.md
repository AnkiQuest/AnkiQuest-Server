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
  ├─ UniFFI → Python + native lib per OS → desktop add-on, rules Python needs
  └─ crates/rules-wasm → one .wasm (rules-<N> release) → desktop add-on's web views
```

## What is shared

Phase 1 is a pilot with one rule: today's day verdict (`tests/fixtures/day_verdicts.json`).

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

The first rule in the WebAssembly module is the review heatmap's colour level
(`intensity_cases` in `tests/fixtures/review_heatmap.json`):

- `heatmap_intensity(count, daily_average) -> u8`: 0 for no reviews, then up to half the reviews
  per study day, up to the average, up to twice it, and more (4). An average under one counts as
  one. Exported through UniFFI too.

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
| `ankiquest-rules-<os>-<arch>` | the library for linux-x86_64, linux-aarch64, windows-x86_64, macos-arm64 or macos-x86_64, `ankiquest_rules.py`, `SHA256SUMS` |
| `ankiquest-rules-wasm` | `ankiquest_rules.wasm`, `SHA256SUMS` |

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
- **Desktop add-on**: ship the five desktop libraries and `ankiquest_rules.py` from the newest rules
  release, load the one for the running platform, and swap the Python rule. Measure on Anki 24.11
  and the newest Anki on Linux (x86_64 and aarch64), Windows and macOS (arm64 and x86_64), and the
  AnkiWeb package size before and after.

Clients take the bytes from the newest `rules-<N>` release (below), checked by SHA-256, not from workflow
artifacts, which expire after 90 days; building from source in every client CI would repeat the
toolchain and platform matrix for the same bytes. Building from source stays possible, and
reproducible, for anyone who wants to check. The Android app does not use the crate today, and its libraries are not released.

## WebAssembly for web views

`crates/rules-wasm` (`ankiquest-rules-wasm`) builds the crate for `wasm32-unknown-unknown` as one
module for every operating system and CPU. Like `crates/i18n-wasm` it uses no bindgen: the module
imports nothing, and a page instantiates it with an empty import object and calls the exports with
plain numbers. Rules that take or return text will pass UTF-8 through `alloc` and `dealloc`, as
`crates/i18n-wasm` does.

| Export | Meaning |
| --- | --- |
| `abi_version() -> u32` | 1. Changes whenever an export is removed or changes meaning. |
| `heatmap_intensity(count: u32, daily_average: f64) -> u32` | As above. |

`crates/rules-wasm/build.sh <out>` builds `ankiquest_rules.wasm` and `SHA256SUMS` from a clean
target directory with the pinned toolchain, the `wasm` profile and no machine paths. The `wasm` job
of the `Shared rules` workflow builds it twice, fails unless the bytes match, runs the fixture cases
through it in Node (`crates/rules-wasm/tests/`) and uploads the `ankiquest-rules-wasm` artifact.

### Releases

The `Rules release` workflow (`.github/workflows/rules-release.yml`) runs on `master` pushes that
touch the crates. It calls the `Shared rules` workflow, which builds and tests every form, and when
any asset's bytes differ from the latest `rules-<N>` release it publishes `rules-<N+1>` with:

- `ankiquest_rules.wasm`: the module.
- `ankiquest_rules.py`: the Python bindings.
- `libankiquest_rules-linux-x86_64.so`, `libankiquest_rules-linux-aarch64.so`,
  `ankiquest_rules-windows-x86_64.dll`, `libankiquest_rules-macos-arm64.dylib`,
  `libankiquest_rules-macos-x86_64.dylib`: the desktop libraries, named after their platform. Next
  to `ankiquest_rules.py` a library must carry its plain name (`libankiquest_rules.so`,
  `ankiquest_rules.dll`, `libankiquest_rules.dylib`), which is what the bindings load.
- `SHA256SUMS`: every asset.

Clients use server `master`, not a pinned commit, so they take the newest `rules-<N>` release when
they build or package: they list the releases, take the highest N, download the assets they need,
check each against that release's `SHA256SUMS`, and record the tag in what they build, so a build
can be traced and rebuilt from the same release. A client that needs reproducible builds from its
own source alone (an F-Droid build, say) would pin a release instead; none does today.

## Which form each client uses

One crate, one set of fixtures, two compiled forms: native libraries through UniFFI, and the
WebAssembly module.

**Desktop add-on.** Anki's whole interface is Qt WebEngine pages (the deck list, the reviewer), so
every Anki the add-on supports has a WebAssembly engine; a Python fallback for a missing WebEngine
is never needed. The deck list is a trusted page whose content security policy only sets
`frame-ancestors`, so a page can compile a module.

| | WebAssembly in a web view | UniFFI library through `ctypes` |
| --- | --- | --- |
| Files to ship | one module, a few hundred bytes so far | one library per OS and CPU; Windows on arm64 is not built |
| Runs | inside the page, sandboxed; a fault cannot end Anki | in Anki's process |
| Called from | the page's script, asynchronously | Python, synchronously, any thread |
| Fits | rules whose result a page draws: heatmap, cards, moods on the Today panel | rules Python acts on: day verdicts for notifications and uploads, deck names in Qt dialogs |

So the add-on uses the module for what its pages draw, starting with the heatmap's colour levels,
and the UniFFI libraries, shipped in the add-on for the platforms Anki 24.11 and newer run on, for
rules Python needs synchronously: day verdicts, and deck names in Qt dialogs.

**Android.** The app draws its AnkiQuest views natively, not in a WebView, and its rules run
outside the UI (widgets, workers, notifications).

| | UniFFI and JNA (JNI) | WebAssembly on the JVM (Chicory) | WebAssembly in a WebView |
| --- | --- | --- | --- |
| Bindings | generated Kotlin types | hand-written marshalling | hand-written, plus a JavaScript bridge |
| Calls | synchronous, native speed | synchronous, interpreted or AOT-compiled | asynchronous, needs a WebView, unavailable to workers and widgets |
| Size | one `.so` per ABI plus JNA | the runtime jar plus one module | one module |
| State | built and tested in CI | not tried | not tried |

Android stays on UniFFI: it already builds reproducibly per ABI, the ABI split APKs ship one
library each, and the generated types keep the Kotlin side free of glue code. Chicory remains the
fallback if the native libraries or JNA become a size or F-Droid problem; the module would then
serve Android unchanged.

## Further rules

Streak moods, card labels and deck names move into the crate one at a time, each with its fixture
and, where a page draws the result, an export in the module.
