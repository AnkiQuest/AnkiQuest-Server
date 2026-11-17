# A single source for companion art

The transparent PNG masters live in `static/<companion>/<pose>.png`, and
nowhere else. Every other picture of a companion is made from them by
`build.rs` or an `xtask` exporter and checked in CI; nobody commits a
hand-made copy (AGENTS.md, Companion art).

Pose names and fallbacks come from `tests/fixtures/companion_poses.json`.
Masters named before the fixture keep their file names: `happy` is
`celebrate.png`, `asleep` is `sleeping.png` (`crates/art/src/poses.rs`).
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
has none at the moment; the manifest holds its retired copies.

The desktop add-on's copies (`addon/art/*.png` and `addon/*_face.png`) are
in AnkiQuest-Desktop, listed in the same format in its `art-exports.json`.
`cargo run -p ankiquest-xtask -- export-addon-art <desktop-checkout>`
writes each one whose picture changed, moves the file it replaces to the
checkout's `assets/retired/` (listed as retired), and records each file's
SHA-256; `--check` fails when a file differs from that hash or from a
fresh export of its master.

Linux exports are canonical. Other platforms and quantiser versions move a
few palette entries, so a committed pose is current when a fresh export
differs from it by a mean under 0.5/255 per channel and in under 1% of its
pixels. A clean checkout passes on every platform, and export there leaves
the committed files alone.

The README banner and social preview are drawn by `xtask banner` and
checked by its own tests against a fresh render. Android's WebPs live in
the Android repository, which checks them against the pinned server commit.

## Retired copies

Companion art is never deleted. Copies nothing reads any more are listed
under `retired` in the manifest, each with the reason, and a test fails if
code under `src/`, `web/`, `addon/`, `crates/` or `build.rs` names one:

- `assets/cards/*.png`: the hand-made share-card copies `src/cards.rs` used
  to embed.
- `static/aki/face-192.png`, `static/aki/face-32.png`,
  `static/ankilope/face-192.png`: hand-made icon copies, now exported by
  `build.rs`.
- `docs/branding/ankilope-celebrate-review/review.png` and
  `docs/branding/core-poses-2026-11/aki-core-*.png`: review sheets.

## The guard

`crates/xtask/src/art/guard.rs` walks every PNG, WebP and GIF in the
repository. An image is companion art when its path names a companion (Aki,
the companions in `tests/fixtures/companion_unlock.json`, or a folder in
`static/`), or when its 16 px fingerprint matches a master at another size.
It fails unless such an image is a master in `static/<companion>/`, an
export in the manifest, or retired there. Animated PNGs get no fingerprint:
their frames are not a scaled copy of one still.

Anki-tan's folder holds Shigeyuki's asset pack as published
(`static/anki-tan/README.md` maps its files to poses). It has no
`<pose>.png` masters yet, so share cards show no art for her.

## Icon poses

`ankiquest_art::ICON_POSES` lists small pictures of a companion, exported
for every companion that has the master and for no other:

| Pose | Fallback | Use | Add-on | Android |
|---|---|---|---|---|
| `icon_fire` | `streak` | the companion on fire, beside the streak number | `addon/art/<companion>_icon_fire.png`, 64 px | `<companion>_icon_fire.webp`, 96 px |

The add-on shows the streak beside its flame at up to 32 px at 2x; the
Android widget draws its flame at 12 dp, 48 px at xxxhdpi, and the export
doubles that for larger widgets. `export-addon-art` adds the entry to the
Desktop checkout's manifest when the master appears, and its check fails
until it has. A client that finds no icon shows the fallback from
`tests/fixtures/companion_poses.json`.

## Adding art

- A new master: add `static/<companion>/<pose>.png`. Share cards pick it up
  on the next build when it is a card pose. An icon pose master needs
  `export-addon-art` and `export-android-art` run.
- A new add-on pose: add an `addon` entry to the Desktop checkout's
  manifest and run `export-addon-art <desktop-checkout>`.
- A new Android drawable: add it to `ART` in `crates/xtask/src/art.rs` and
  run `export-android-art`.

Review generated assets at native size and 64 px on light and dark
backgrounds.
