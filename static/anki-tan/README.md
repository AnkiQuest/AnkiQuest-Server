# Anki-tan

Shigeyuki's Anki-tan asset pack v1.0, imported unchanged except for
folder names. Licence and credit: [LICENSE](LICENSE). The pack's `meme`
folder is left out: its pictures reuse other works.

- `emoji/`: 50 chibi faces, 512 px.
- `illust/`, `sticker/`: full-size illustrations (hello, good, main,
  brought cards, press spacebar, let's study).
- `sprite/idle`, `sprite/hello`: 24-frame animations, 720 px, plus 240 px
  animated PNGs.
- `icon.png`: app icon.

## Core poses (AGENTS.md, Companion art)

The pack's art serves as Anki-tan's poses until we draw our own, or for
good. Where the full version has no picture, it uses the chibi face.

The poses each version uses are listed in [`poses.json`](poses.json) and
[`chibi/poses.json`](chibi/poses.json); `xtask art-coverage` and CI read
those lists, so change them with this table. They use the generated
masters below for asleep, sad, neglected and frozen.

| Core pose | Anki-tan | Anki-tan chibi |
|---|---|---|
| face | `sprite/idle/anki-tan_idle_0001.png` | `emoji/Ankitan_00_smile.png` |
| asleep | `emoji/Ankitan_35_sleeping.png` | `emoji/Ankitan_35_sleeping.png` |
| happy | `sticker/anki_tan_good.png` | `emoji/Ankitan_10_party.png` |
| streak | `emoji/Ankitan_09_fire_eyes.png` | `emoji/Ankitan_09_fire_eyes.png` |
| sad | `emoji/Ankitan_02_cry.png` | `emoji/Ankitan_02_cry.png` |
| hopeful | `sticker/ankitan_lets_study.png` | `emoji/Ankitan_06_pleading.png` |
| frozen | `frozen.png` (generated) | `chibi/frozen.png` (generated) |
| neglected | `emoji/Ankitan_49_lost_streaks.png` | `emoji/Ankitan_49_lost_streaks.png` |

Extensions, both versions unless noted:

| Extension | File |
|---|---|
| welcome | `sticker/anki_tan_hello.png` (full), `emoji/Ankitan_21_saluting.png` (chibi) |
| review | `sticker/press_spacebar.png` |
| streak_legend | `emoji/Ankitan_43_crown.png` |
| streak_milestone | `emoji/Ankitan_48_perfect_streaks.png` |
| hurt | `emoji/Ankitan_28_spiral_eyes.png` |
| dead | `emoji/Ankitan_39_crossed_out_eyes.png` |
| celebrate (unlock reveal) | `illust/anki_tan_main.png` |
| idle and wave animations | `sprite/idle`, `sprite/hello` |

Launcher icons (full version): `icon` is `icon.png`; `icon_happy`,
`icon_worried` and `icon_sleepy` are `emoji/Ankitan_09_fire_eyes.png`,
`emoji/Ankitan_02_cry.png` and `emoji/Ankitan_35_sleeping.png`; `icon_frozen`
is not drawn and shows `icon`.

## Generated additions

The canonical generated masters are `asleep.png`, `sad.png`,
`neglected.png`, `frozen.png` and `chibi/frozen.png`. Their earlier names
under `generated/` remain unchanged for preservation. No imported
Shigeyuki file was replaced. The other contributor owns activation;
the table above records the imported pack's original mapping.

These derivatives were generated with OpenAI `image_gen` (model identifier
not exposed) and are CC BY-SA 4.0 under [LICENSE](LICENSE).
Credit: Anki-tan by Shigeyuki (https://www.patreon.com/Shigeyuki).
See [provenance and approval](../../docs/branding/ankitan-full-2026-11/README.md).
