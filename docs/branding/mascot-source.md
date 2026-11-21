# A single source for companion art

The transparent PNG masters live in
[AnkiQuest-Art](https://github.com/AnkiQuest/AnkiQuest-Art), which this
repository takes as the git submodule `art/` (`ankiquest_art::MASTERS`).
A master is `art/<companion>/<pose>.png`, one file per pose, and Art's
`manifest.json` lists every one with its companion and licence. Art's
`archive/` keeps pictures that are not masters and its `docs/` the
generation records and review sheets; nothing here reads either as a
master. No picture of a companion is committed to this repository: every
copy is made from the masters by `build.rs` or an `xtask` exporter and
checked in CI (AGENTS.md, Companion art).

Check out the submodule with `git clone --recurse-submodules`, or
`git submodule update --init` in an existing checkout. A new or changed
master lands in AnkiQuest-Art first; this repository picks it up by moving
the submodule to the new commit.

Pose names and fallbacks come from `tests/fixtures/companion_poses.json`:
the core moods, the moment and share-card pictures and the launcher icon
moods a companion needs to go live, and the extensions. `freeze.png` (a used
streak freeze) is the `freeze` picture, not the core `frozen` pose.
`study.png` (Ankilope) and `icon.png` (Anki-tan) keep names the fixture does
not list. Art's `RENAMES.md` says where each picture that was in `static/`
is now.

Wins store the art name `celebrate` in their `art` field, from before the
pose list; `src/cards.rs` shows the `happy` pose for it, and `src/aki.rs`
serves `/aki/celebrate.png` and `/ankilope/celebrate.png` from the happy
masters, so released apps and stored wins keep working.

```text
art/<companion>/<pose>.png
    |
    +-- build.rs (crates/art) --> OUT_DIR: share-card art, face icons, Anki-tan's web pictures
    |
    +-- src/aki.rs --> served as is: Aki's and Ankilope's web pictures
    |
    +-- xtask export-addon-art --> Desktop addon/art/*.png, addon/*_face.png
    |
    +-- xtask export-android-art --> Android drawable-nodpi WebPs
    |
    +-- xtask banner --> docs/graphics/banner_readme.png, social_preview.png
```

## Shared exporter

`crates/art` (`ankiquest-art`) holds the downscaler every export uses
(Lanczos in premultiplied alpha, never upscaled), the PNG encoder, the pose
list with its fallbacks and a reader for Art's `manifest.json`. `build.rs`
and `crates/xtask` both call it.

## Server

`build.rs` writes these under Cargo's `OUT_DIR/art`, and the server embeds
them from there:

- Share-card art, 512 px: for Aki, Anki-tan and Ankilope, each card pose
  they have (`happy`, `streak`, `frozen`, `freeze`, `winner`), listed in a
  generated `cards.rs`. `src/cards.rs` picks the pose, else its fallback
  from the fixture, else `happy`; a companion without masters shows no art.
  Bluma's cards still show Ankilope's art turned purple (#88).
- Face icons: `aki/face-192`, `aki/face-32`, `ankilope/face-192` and
  `anki-tan/face-192`, served at `/aki/face-192.png` and so on.
- Anki-tan's web pictures at 320 px, served at `/anki-tan/<pose>.png`.

Nix builds with `inputs.self.submodules = true`, so the flake's source
includes `art/` and `nix build` runs the same export.

## Desktop copies

The desktop add-on's copies (`addon/art/*.png` and `addon/*_face.png`) are
in AnkiQuest-Desktop, listed in its `art-exports.json`, where each entry's
`master` is a path in AnkiQuest-Art, such as `aki/happy.png`.
`cargo run -p ankiquest-xtask -- export-addon-art <desktop-checkout>`
writes each one whose picture changed, moves the file it replaces to the
checkout's `assets/retired/` (listed as retired), and records each file's
SHA-256;
`--check` fails when a file differs from that hash or from a fresh export
of its master.

Linux exports are canonical. Other platforms and quantiser versions move a
few palette entries, so a committed pose is current when a fresh export
differs from it by a mean under 0.5/255 per channel and in under 1% of its
pixels. A clean checkout passes on every platform, and export there leaves
the committed files alone.

The README banner and social preview are drawn by `xtask banner` and
checked by the CI `screenshots` job against a fresh render. Android's WebPs
live in the Android repository, which checks them against the pinned
server commit.

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
the gallery reads ([art-coverage.md](art-coverage.md)). CI runs `--since`
with the base commit's fixture: an item added to the list needs a master for
every live companion. Gaps the list already had do not fail; those
companions show the fixture's fallbacks (hill, 2026-11-21).

## Adding art

- A new master: add `<companion>/<pose>.png` to AnkiQuest-Art and its
  `manifest.json`, then move the submodule here. Share cards pick it up on
  the next build when it is a card pose.
- A new add-on pose: add an `addon` entry to the Desktop checkout's
  manifest and run `export-addon-art <desktop-checkout>`.
- A new Android drawable: add it to `ART` in `crates/xtask/src/art.rs` and
  run `export-android-art`.

Review generated assets at native size and 64 px on light and dark
backgrounds.
