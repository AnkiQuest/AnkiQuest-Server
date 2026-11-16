# Agent instructions

AnkiQuest-Server is the AnkiQuest server (Rust), its web pages (TypeScript), the shared fixtures and the companion art masters. The clients live in [AnkiQuest/AnkiQuest-Android](https://github.com/AnkiQuest/AnkiQuest-Android) and [AnkiQuest/AnkiQuest-Desktop](https://github.com/AnkiQuest/AnkiQuest-Desktop) (the Anki desktop add-on). These rules apply to every agent (Claude, Codex, others) and to people too.

## Maintaining the code

Work on this codebase as its long-term maintainer would; its design is up to you.

Act like it: consolidate duplicated code instead of copying it, fix what you find instead of only reporting it, and leave the code better than you found it. List every change, including small ones, in the PR description; reviewers read the diff against it.

The project is run by hill (GitHub `float3`) and Aldana (GitHub `AldanaBRZ`). Refer to each of them by name or GitHub user, never as "the owner". "The owners" means both of them together. When you cite an approval, name who gave it and link it.

- Don't hold on to the past. Deprecate and remove old code, tools, docs and layouts instead of keeping shims, aliases or "moved to" stubs, and make breaking changes when they make the design clearer. Say in the PR what breaks and who has to act. The rules on released apps and companion art below still apply.

## Bugs: test first

1. Write a regression test that reproduces the bug.
2. Run it and watch it fail for the expected reason.
3. Fix the bug.
4. Run it again and watch it pass, together with the rest of the suite.

Say in the PR that the test failed before the fix. Never skip, disable or loosen a test to get CI green.

## Languages

Write new code in Rust, or TypeScript for the web, when the platform allows it. Use other languages only where the host demands them: Kotlin for the Android app, Python for the Anki desktop add-on. Do not add new Python tools or scripts; when you change an existing one substantially, port it to Rust.

## Code

- Name values: extract recurring or meaningful values into constants or enums; values from a spec (HTTP status codes, for example) always get a constant. Keep obvious one-off values inline.
- Return early; avoid deep nesting.
- Always use braces, even for one-line `if`s.
- Short function names, under 30 characters.
- Enums instead of booleans for function parameters.
- Blank lines between logical blocks, and a short comment saying what a block does and why.
- Keep members private. Widening visibility (private to internal or public) needs explicit approval from hill or Aldana; name who approved and link to it in the PR. Never claim an approval you can't link.
- Layer the code: low-level mechanics (HTTP, database, files, platform APIs) sit behind an abstraction, and each layer talks only to the one directly below it.
- Prefer targeted edits over rewriting whole files.
- Keep all state explicit. Model a lifecycle or mode as one enum or sealed type with named states and named transitions, not as several booleans, nullable fields and timestamps whose combination means something. Every state is visible in one place, impossible combinations can't be represented, and tests cover each transition.

## Git and pull requests

- Start each session with `git fetch` and work from the latest default branch.
- Never push to the default branch; open a pull request and merge only when every check is green on its final commit.
- Commit messages: subject in the imperative, capitalised, at most 50 characters (72 hard limit), no full stop; a blank line; a body wrapped at 72 characters that explains what and why, not how.
- Name the model that did the work. Every commit, pull request, issue, comment and review an agent writes ends with `Assisted-by: <tool> (<model and version>)`, such as `Assisted-by: Claude Code (Opus 5.5)` or `Assisted-by: Codex (GPT-6)`. Generated images name the model that drew them in the pull request and in their generation record. If the tool does not say which model it used, write that instead of guessing.
- Never link to an agent session (such as `claude.ai/code/session_…`) in commits, pull requests, issues or comments.
- No emoji in commits, pull requests, issues or comments.
- Never commit secrets, tokens or personal data. Secrets live on the servers, outside the repositories.

## Desktop and Android parity

AnkiQuest has two clients: the Android app ([AnkiQuest/AnkiQuest-Android](https://github.com/AnkiQuest/AnkiQuest-Android)) and the Anki desktop add-on (`addon/` in [AnkiQuest/AnkiQuest-Desktop](https://github.com/AnkiQuest/AnkiQuest-Desktop)). A player should get the same AnkiQuest on both.

- A user-facing feature or behaviour change lands on both clients, unless the platform can't do it (home-screen widgets, for example). Do both in one go when you can. Otherwise open an issue in the other repository, labelled `parity`, and link it from the PR.
- Rules both clients compute (streak verdicts, heatmap, card labels, deck names) are defined once, as cases in `tests/fixtures/*.json` in AnkiQuest/AnkiQuest-Server. Both clients test against the same cases. Change the fixture first, then both implementations.
- Use the same names, wording and order for the same thing on both clients.
- When you find a gap between the clients, open a `parity` issue for it.

## Companion art

Companion art means every picture of a character: poses, moment and card pictures, widget and notification drawables.

- Never delete companion art, even when nothing uses it any more. Remove the references, keep the files, so they can be reused or merged later.
- Each character needs a core set of poses before it goes live in the apps: face, asleep, happy, streak (on fire), sad, hopeful, frozen and neglected. A character without the full core set stays hidden.
- Every other pose is an extension: streak tiers, milestones, frozen days 2 and 3, hurt, dead, welcome, a defeated leech, and poses for other rare moments. Extensions are optional. A missing extension falls back to its core pose (a streak tier to streak, frozen 3 to frozen, a defeated leech to happy).
- The core set, the extensions and their fallbacks are defined once, in `tests/fixtures/companion_poses.json`; both clients use that list.

## Writing

Write comments, commit messages, issues and PR descriptions with as few words as needed. No praise, no filler, no superlatives. Em dashes take no spaces around them.

Mannered prose substitutes metaphor and flourish for direct statement. Instead of "a parameter worth varying," the mannered writer produces "a dial worth turning." Instead of "this point still matters," they write "this point earns its keep." The phrases exist to display the writer, not to convey the idea, and readers can tell. That is why mannered prose irritates: it makes the reader work harder so the writer can perform. It is also imprecise. Metaphors drag in connotations the writer did not choose and cannot control. The fix is to say what you mean. When a literal phrase is available, use it.

## This repository

- **Layout.** `src/` and `crates/`: the Rust server. `web/`: TypeScript for the pages in `static/`. `static/`: also the companion art masters. `tests/`: web tests (TypeScript) and the shared fixtures in `tests/fixtures/`. `crates/xtask` holds the repository tools (`cargo run -p ankiquest-xtask -- <command>`), including the art exporters for an Android or Desktop checkout. This repository has no Python; the add-on and its tests are in AnkiQuest/AnkiQuest-Desktop.
- **API examples are a contract.** `docs/api/examples/*.json` and `tests/fixtures/*.json` are copied into the Android app at a pinned commit (AnkiQuest/AnkiQuest-Android#203); AnkiQuest-Desktop is to pin it the same way. Adding an example is fine: clients pick it up through their bump PRs. Never remove or reshape an example released apps read (#230); retire it only after Android stops reading it. Document API changes in `docs/mobile-api.md`.
- **Released apps talk to the deployed server.** Keep old endpoints and fields working until no supported client uses them.
- **Deployment** is the NixOS module in `flake.nix`, used from a separate NixOS configuration. Secrets are files on the hosts (passed with systemd `LoadCredential`), never values in this repository.
- **Checks to run** before pushing, for what you changed: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run typecheck`, `npm run test:web`. Leave the full matrix to CI.
