# Art coverage as JSON

`cargo run -p ankiquest-xtask -- art-coverage --json` prints the art matrix
(AGENTS.md, Companion art): every companion by every item of
`tests/fixtures/companion_poses.json`, with what each shows, the licence of
its pictures and the archive. The gallery (AnkiQuest.github.io) builds from
it on AnkiQuest-Server `master`. Paths are relative to the repository root
and start with `art/`, the AnkiQuest-Art submodule (`ankiquest_art::MASTERS`):
`art/<path>` is
`https://raw.githubusercontent.com/AnkiQuest/AnkiQuest-Art/<commit>/<path>`,
where `<commit>` is the submodule's (`git rev-parse HEAD:art`).

`schema` goes up when a field changes meaning or goes away; added fields
keep it.

```jsonc
{
  "schema": 1,
  "fixture": "tests/fixtures/companion_poses.json",

  // The kinds of item; `core` kinds are what a companion needs to go live.
  "kinds": [
    {"kind": "core_mood", "core": true},
    {"kind": "picture", "core": true},
    {"kind": "icon", "core": true},
    {"kind": "extension", "core": false}
  ],

  // Every item once, in kind order: the matrix's rows. `fallback` is the
  // item a companion without this one shows; null for a core mood.
  "items": [
    {"name": "face", "kind": "core_mood", "fallback": null},
    {"name": "place_1", "kind": "picture", "fallback": "winner"},
    {"name": "icon_happy", "kind": "icon", "fallback": "icon"},
    {"name": "frozen_2", "kind": "extension", "fallback": "frozen"}
  ],

  // Licences by key; every picture of a companion or archive folder has
  // its folder's licence. `credit` is the attribution the licence requires.
  "licences": {
    "repository": {"spdx": "AGPL-3.0-or-later", "file": "art/LICENSE", "credit": null},
    "anki-tan": {"spdx": "CC-BY-SA-4.0", "file": "art/anki-tan/LICENSE",
                 "credit": "Anki-tan by Shigeyuki (https://www.patreon.com/Shigeyuki)"}
  },

  // The matrix's columns, in picker order.
  "companions": [
    {
      "id": "anki_tan",
      "name": "Anki-tan",
      "folder": "art/anki-tan",
      "stage": "live",                   // or "planned"
      "licence": "anki-tan",             // a key of `licences`
      // One entry per item, in the order of `items`.
      "items": [
        {"name": "face", "kind": "core_mood", "status": "present",
         "file": "art/anki-tan/face.png"},
        {"name": "icon_frozen", "kind": "icon", "status": "fallback",
         "shows": "icon", "file": "art/anki-tan/icon.png"},
        {"name": "x", "kind": "picture", "status": "missing"}
      ],
      // What is missing, as the backlog: core items and extensions.
      "missing": {"core": ["achievement", "..."], "extensions": ["frozen_2", "..."]},
      // PNGs directly in the folder that no item names, such as Ankilope's
      // study.png.
      "unlisted": []
    }
  ],

  // Archive folders, apart from the masters: each folder of AnkiQuest-Art's
  // `archive/`. Replaced masters, earlier copies and Shigeyuki's source
  // pack; never exported to a client.
  "archive": [
    {"folder": "art/archive/cow", "licence": "repository",
     "readme": "art/archive/cow/README.md",
     "files": ["art/archive/cow/face.png", "..."]}
  ]
}
```

A `fallback` status names the item shown and its file: a missing item shows
the first item its chain reaches that the companion has. `missing` shows
nothing; a client shows no picture there. Why an item is missing is the
issue that tracks its drawing (AnkiQuest/AnkiQuest-Server#57 for poses, #116
for icons and moment pictures).
