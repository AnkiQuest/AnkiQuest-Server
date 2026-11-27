# A single source for companion art

The transparent PNG masters live in
[AnkiQuest-Art](https://github.com/AnkiQuest/AnkiQuest-Art), which this
repository takes as the git submodule `art/` (`ankiquest_art::MASTERS`,
the one place the path is named). Art's `manifest.json` lists every master
with its companion, pose, file and licence; this repository finds each
master through it, by companion and pose, never by file name. Several poses
may name the same file (Anki-tan chibi's `idle.png`). Art's `archive/`
keeps pictures that are not masters and its `docs/` the generation records
and review sheets; nothing here reads either as a master. No picture of a
companion is committed to this repository: every copy is made from the
masters by `build.rs` (AGENTS.md, Companion art).

Check out the submodule with `git clone --recurse-submodules`, or
`git submodule update --init` in an existing checkout. A new or changed
master lands in AnkiQuest-Art first; this repository picks it up by moving
the submodule to the new commit.

Pose names and fallbacks come from the pinned AnkiQuest-Art release's `poses.json` (staged into `art/`):
the core moods, the moment and share-card pictures and the launcher icon
moods a companion needs to go live, and the extensions. Until their
masters are drawn, the core moods `happy` and `streak` show `hopeful`
(`CORE_FALLBACKS` in `crates/art`, as AnkiQuest-Art's pose list will give
them). `freeze.png` (a used streak freeze) is the `freeze` picture, not the
core `frozen` pose. Art's `RENAMES.md` says where each picture that was in
`static/` is now.

Wins store the art name `celebrate` in their `art` field, from before the
pose list; `src/cards.rs` shows the `happy` pose for it, and the server
serves `/aki/celebrate.png` and `/ankilope/celebrate.png` from the happy
masters, so released apps and stored wins keep working.

```text
art/manifest.json --> art/<companion>/<pose>.png
    |
    +-- build.rs (crates/art) --> OUT_DIR/art: share-card art and every
                                  picture src/aki.rs serves
```

The clients' copies (add-on art, Android drawables) are exported by
AnkiQuest-Art's own tool (`tools/art-export`), which AnkiQuest-Desktop and
AnkiQuest-Android run at build time. The README banner and social preview
are drawn by AnkiQuest-Dev's `ankiquest-banner` from
`docs/graphics/banner.toml`, whose pose is `art/duo/friends.png`.

## Shared code

`crates/art` (`ankiquest-art`) holds the downscaler every export uses
(Lanczos in premultiplied alpha, never upscaled), the PNG encoder, the pose
list with its fallbacks and the reader for Art's `manifest.json`.
`build.rs` and `crates/xtask` both call it.

## Server

`build.rs` writes these under Cargo's `OUT_DIR/art`, and the server embeds
them from there:

- Share-card art, 512 px, for Aki, Anki-tan and Ankilope: for each card
  pose (`happy`, `streak`, `frozen`, `freeze`, `winner`) the master, or the
  one its fallbacks reach, listed in a generated `cards.rs`. `src/cards.rs`
  picks the pose, else its fallback, else `happy`; a companion without
  masters shows no art. Bluma's cards still show Ankilope's art turned
  purple (#88).
- Every picture the pages ask for, listed in a generated `pictures.rs`
  that `src/aki.rs` serves: Aki's and Ankilope's masters as they are, the
  face icons (`face-192`, `face-32`) and Anki-tan's pictures at 320 px. A
  picture whose pose has no master shows the one its fallbacks reach;
  Anki-tan's `welcome`, `review`, `happy` and `streak` show her `hopeful`
  master until those are drawn.

Nix builds with `inputs.self.submodules = true`, so the flake's source
includes `art/` and `nix build` runs the same export.

## The guard

`crates/xtask/src/art/guard.rs` walks every PNG, WebP and GIF in the
repository outside `art/`. An image is companion art when its path names a
companion (Aki, the companions in `tests/fixtures/companion_unlock.json`,
or one in Art's manifest), or when its 16 px fingerprint matches a master
at the same or a smaller size. Any such image fails the test. Animated PNGs
get no fingerprint: their frames are not a scaled copy of one still.

## Coverage

`cargo run -p ankiquest-xtask -- art-coverage` prints which companion has
which item of the fixture, `--missing` the backlog, and `--json` the matrix
the gallery reads ([art-coverage.md](art-coverage.md)); its file paths are
paths in this checkout, below `art/`. CI runs `--since` with the base
commit's fixture: an item added to the list needs a master for every live
companion. Gaps the list already had do not fail; those companions show the
fallbacks.

## Adding art

- A new master: add it to AnkiQuest-Art and its `manifest.json`, then move
  the submodule here. Share cards and served pictures pick it up on the
  next build.
- A new served picture: add it to `PICTURES` in `build.rs`.
- A new client copy: add it to AnkiQuest-Art's `tools/art-export`.

Review generated assets at native size and 64 px on light and dark
backgrounds.
