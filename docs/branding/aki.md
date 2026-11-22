# Aki, the AnkiQuest study companion

Aki is an orange-and-cream red panda with an indigo scarf, a flashcard satchel,
and a ringed tail. the original artwork was generated for AnkiQuest with
OpenAI's image-generation tool and approved by the project owner. production
prompts are recorded beside this document. the PNG originals have transparent
backgrounds and are approximately 1280 × 1280 pixels.

## where Aki appears

- website and installed web-app branding: navigation, sign-in, favicon and icon.
- profiles: encouragement reflecting today's actual quests and streak state.
- community, records and trophies: headers, loading and empty states.
- streak protection: a cozy freeze illustration in its settings dialog.
- desktop add-on: a locally served deck-list image and settings dialog icon.

the seven assets in the table below are embedded in the server binary. only their
exact public paths bypass sign-in; member APIs and unknown paths remain
protected. the desktop face is packaged locally in AnkiQuest-Desktop's `addon/aki_face.png` so
branding works offline.

the Android app ships WebP copies of the Aki and Ankilope art in `drawable-nodpi`. after changing a PNG in `static/`, run `cargo run -p ankiquest-xtask -- export-android-art <android-checkout>` to rewrite them, and `--check` to confirm every drawable is byte for byte what export writes (export on Linux; CI checks there).

the desktop add-on ships 256 px palette PNGs of the poses it shows in `addon/art` of AnkiQuest-Desktop, and its two faces in `addon/*_face.png`; that checkout's `art-exports.json` lists them. `cargo run -p ankiquest-xtask -- export-addon-art <desktop-checkout>` overwrites those whose picture changed, deletes those whose master is gone, and records their SHA-256, and `--check` fails while one is missing, edited, has no master, or differs from a fresh export of its master. Which pose shows for today's streak, and which line the Today panel and Home card show with it, is one rule for both clients, pinned by `tests/fixtures/streak_moods.json`; it names poses from `tests/fixtures/companion_poses.json`, and each client draws them with the chosen companion's art.

the duo also leads the README banner (`docs/graphics/banner_readme.png`) and the repository's social preview (`docs/graphics/social_preview.png`). AnkiQuest-Dev's `ankiquest-banner` draws both from the duo and the website screenshots in `docs/screenshots/web/` (`docs/graphics/banner.toml`); the CI `banner` job fails while either is stale. AnkiQuest-Desktop and AnkiQuest-Android draw theirs with the same tool and the same duo master, with their own screens.

[A single source for companion art](mascot-source.md) describes how every
copy is made from the masters and checked.

## pose and voice rules

| Asset | Use |
| --- | --- |
| `welcome.png` | Onboarding, sign-in and empty states |
| `review.png` | A small next step, loading and studying |
| `celebrate.png` | Confirmed completed daily quests and conquered cards |
| `streak.png` | Studying today and general progress |
| `freeze.png` | Confirmed protection; contextual settings illustration |
| `winner.png` | Records, trophies, winners and tamed leeches |
| `face.png` | Compact branding and installed app icon |
| `face-192` and `face-32` | `face.png` scaled down by `build.rs` for the website's brand mark, touch icon and favicon (also `ankilope/face-192`) |

### moment art (#88)

these PNGs (1254 × 1254, transparent) cover moments that the seven poses above
only approximated. they are source art for the Android app's `drawable-nodpi`
names in brackets, written by the export tool. the website does not serve them
yet: they are not embedded in the server binary and are not public paths.

| Asset | Android name | Use |
| --- | --- | --- |
| `icon_happy.png` | `aki_icon_happy` | App icon mood: today counts |
| `icon_worried.png` | `aki_icon_worried` | App icon mood: streak ends soon, today doesn't count yet |
| `icon_frozen.png` | `aki_icon_frozen` | App icon mood: a freeze protects the streak |
| `icon_sleepy.png` | `aki_icon_sleepy` | App icon mood: no streak running |
| `streak_hot.png` | `aki_streak_hot` | Streak of 7+ days |
| `streak_blazing.png` | `aki_streak_blazing` | Streak of 30+ days |
| `streak_legend.png` | `aki_streak_legend` | Streak of 100+ days |
| `streak_milestone.png` | `aki_streak_milestone` | A streak milestone reached |
| `freeze_used.png` | `aki_freeze_used` | A freeze just saved the streak |
| `comeback.png` | `aki_comeback` | Starting again after a lost streak |
| `minimum.png` | `aki_minimum` | Daily minimum reached; today counts |
| `goal.png` | `aki_goal` | Daily goal reached |
| `achievement.png` | `aki_achievement` | An achievement unlocked |
| `level_up.png` | `aki_level_up` | A level reached |
| `deck_conquered.png` | `aki_deck_conquered` | A deck conquered |
| `leech.png` | `aki_leech` | A leech tamed |
| `at_risk.png` | `aki_at_risk` | Still time to secure the streak |
| `sad.png` | `aki_sad` | Streak mood: a streak and no study yet today |
| `hopeful.png` | `aki_hopeful` | Streak mood: half of today's minimum studied; also the `almost` mood (three quarters) |
| `frozen.png` | `aki_frozen` | Streak mood: a freeze kept the days before today (1, 2 or 3+ days), under a quarter of today's minimum studied; also the `thawing` mood (a quarter) |
| `neglected.png` | `aki_neglected` | Core pose for a long absence; no streak mood shows it yet |
| `backlog.png` | `aki_backlog` | Calmly sorting overdue flashcards, one small step at a time |
| `place_1.png` | `aki_place_1` | Weekly first place, gold medal and a friendly wave |
| `place_2.png` | `aki_place_2` | Weekly second place, proudly holding a silver medal |
| `place_3.png` | `aki_place_3` | Weekly third place, a relaxed smile and bronze medal |
| `nudge.png` | `aki_nudge` | A friendly wave to another player |
| `challenge.png` | `aki_challenge` | An invitation to a friendly challenge |
| `challenge_won.png` | `aki_challenge_won` | A completed challenge, celebrating as a good sport |
| `challenge_lost.png` | `aki_challenge_lost` | Applauding the other player; available for an explicit losing result |
| `asleep.png` | `aki_asleep` | Rest day, notifications off |
| `../duo/friends.png` | `duo_friends` | Aki and Ankilope together: sharing, challenges, nudges |

The four social poses (`nudge`, `challenge`, `challenge_won`, `challenge_lost`)
also have matching PNGs in `static/ankilope/`, exported under the same names
with the `ankilope_` prefix. Both mascots stay encouraging after a loss.
The [review sheet](social-poses-review.png) shows each at 128 and 64 pixels
on light and dark backgrounds. All eight transparent masters were also
reviewed at native resolution. They were generated with the built-in
`image_gen` tool; the prompt record includes the nudge cutout refinement.

The backlog pose also has a matching `static/ankilope/backlog.png`, exported
as `ankilope_backlog`. Both show a calm companion sorting blank flashcards.
The [backlog review sheet](backlog-review.png) shows each at 128 and 64 pixels
on light and dark backgrounds. Both transparent masters were also reviewed
at native resolution on both backgrounds. Prompts and references are recorded
in `aki-prompts.json`. These assets are ready for backlog-specific surfaces;
adding them does not change study goals or the current general study artwork.

The three podium poses have matching `static/ankilope/place_1..3.png` masters
and `ankilope_place_1..3` Android exports. Gold, silver and bronze medals use
distinct positive poses. The [podium review sheet](podium-review.png) shows
all six at 128 and 64 pixels on light and dark backgrounds. Each 1254px
transparent master was also reviewed at native size on both backgrounds.
Exact generation prompts and references are recorded in `aki-prompts.json`.
Android exports are 336px for the 112dp moment-card illustration; poses the
big streak widget shows are 1024px (`crates/xtask/src/art.rs`).

`aki-prompts.json` records the prompts for new and replaced art. Prompts for
earlier poses without a generation record were reconstructed from the shipped
PNG masters. Each client-sized copy uses the prompt for its master pose.

Aki encourages small steps and celebrates confirmed progress. avoid guilt,
threats, fake rewards or implying a streak is protected merely because a user
owns freezes. an expired daily profile receives neutral study encouragement.
contextual artwork on a settings dialog is decorative, not a protection status.

artwork has empty alternative text and is excluded from interactive controls.
website encouragement is translated into English, Spanish, French, German and
Portuguese; the desktop add-on currently translates English and Spanish.
useful encouragement is ordinary translated text. keep images static, allow
text to wrap, preserve clear space and retain player avatars and names.

Native Android assets ship with the companion Android change, so the app does
not require a network request to display Aki. no review weights, permissions,
scoring, notification timing or user preferences are changed by this branding.
