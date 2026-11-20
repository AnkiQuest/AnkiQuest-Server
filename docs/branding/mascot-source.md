# A single source for companion art

The transparent PNG masters live in `static/<companion>/<pose>.png`, and
nowhere else. Every other picture of a companion is made from them by
`build.rs` or an `xtask` exporter and checked in CI; nobody commits a
hand-made copy (AGENTS.md, Companion art).

Pose names and fallbacks come from `tests/fixtures/companion_poses.json`.
Masters named before the fixture keep their file names: `happy` is
`celebrate.png` (`crates/art/src/poses.rs`). Every `asleep` master is
`asleep.png`; Aki's and Ankilope's were `sleeping.png` until #32.
`freeze.png` (a used streak freeze) is not the core `frozen` pose. Wins store those file names in their `art`
field.

```text
static/<companion>/<pose>.png
    |
    +-- build.rs (crates/art) --> OUT_DIR: share-card art, face icons
    |
    +-- xtask export-addon-art --> Desktop addon/art/*.png, addon/*_face.png
    |
    +-- xtask export-android-art --> Android drawable-nodpi WebPs
    |
    +-- xtask banner --> docs/graphics/banner_readme.png, social_preview.png
```

## Shared exporter

`crates/art` (`ankiquest-art`) holds the downscaler every export uses
(Lanczos in premultiplied alpha, never upscaled), the PNG encoder and the
pose list with its fallbacks. `build.rs` and `crates/xtask` both call it.

## Server

`build.rs` writes these under Cargo's `OUT_DIR/art`, and the server embeds
them from there:

- Share-card art, 512 px: for every folder in `static/` with a `happy`
  master, each card pose it has (`happy`, `streak`, `frozen`, `freeze`, `winner`),
  listed in a generated `cards.rs`. `src/cards.rs` picks the pose, else its
  fallback from the fixture, else `happy`; a companion without masters shows
  no art. The cow has no masters yet (#88) and shows Ankilope's art turned
  purple.
- Face icons: `aki/face-192`, `aki/face-32` and `ankilope/face-192`, served
  at `/aki/face-192.png` and so on.

Nix builds from `lib.cleanSource`, which includes `static/`, `crates/art`
and the fixture, so `nix build` runs the same export.

## Committed copies

`crates/xtask/art-exports.json` lists the copies that must be committed
because a consumer reads them straight from a checkout. This repository
has none at the moment.

The desktop add-on's copies (`addon/art/*.png` and `addon/*_face.png`) are
in AnkiQuest-Desktop, listed in the same format in its `art-exports.json`.
`cargo run -p ankiquest-xtask -- export-addon-art <desktop-checkout>`
overwrites each one whose picture changed and records each file's
SHA-256. An entry whose master is gone (removed or renamed) is deleted:
export removes the file and drops the entry, and a renamed pose needs a
new entry under its new name. `--check` fails when a file differs from
its hash or from a fresh export of its master, or has no master.

Linux exports are canonical. Other platforms and quantiser versions move a
few palette entries, so a committed pose is current when a fresh export
differs from it by a mean under 0.5/255 per channel and in under 1% of its
pixels. A clean checkout passes on every platform, and export there leaves
the committed files alone.

The README banner and social preview are drawn by `xtask banner` and
checked by its own tests against a fresh render. Android's WebPs live in
the Android repository, which checks them against the pinned server commit.

## Retired art

A master nothing uses any more moves to `static/retired/<companion>/`; it
is never deleted, so it can be reused or merged later. No exporter, page or
server route reads that folder, and `build.rs` skips it. It holds Aki's and
Ankilope's old hand-made face icons, Anki-tan's earlier generated poses and
Bluma's earlier masters, each folder with a README where it has one.
Exported copies elsewhere are generated: a copy that was renamed, retired
or is no longer used is deleted with its references, never kept in a
`retired/` folder of a client or listed as unused.

## The guard

`crates/xtask/src/art/guard.rs` walks every PNG, WebP and GIF in the
repository. An image is companion art when its path names a companion (Aki,
the companions in `tests/fixtures/companion_unlock.json`, or a folder in
`static/`), or when its 16 px fingerprint matches a master at another size.
It fails unless such an image sits in `static/<companion>/` or
`static/retired/<companion>/`, or is an export in the manifest. Animated PNGs get no fingerprint:
their frames are not a scaled copy of one still.

Anki-tan's folder holds Shigeyuki's asset pack as published
(`static/anki-tan/README.md` maps its files to poses). It has no
`<pose>.png` masters yet, so share cards show no art for her.

## Adding art

- A new master: add `static/<companion>/<pose>.png`. Share cards pick it up
  on the next build when it is a card pose.
- A new add-on pose: add an `addon` entry to the Desktop checkout's
  manifest and run `export-addon-art <desktop-checkout>`.
- A new Android drawable: add it to `ART` in `crates/xtask/src/art.rs` and
  run `export-android-art`.

Review generated assets at native size and 64 px on light and dark
backgrounds.
