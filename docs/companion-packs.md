# Shared companion packs

A player who imported a companion pack (AnkiQuest-Art `docs/packs.md`) can
upload it, and their friends then see that companion. The server checks
every upload with the app's rules, rebuilds the pack from decoded pixels
and stores the result under its SHA-256. Friends' clients fetch a pack by
that hash. Capability: `companion_packs`.

## What the server stores

| Table | Rows |
|---|---|
| `player_packs` | One per pack a player uploaded: the canonical zip, its SHA-256 and size, and the fields of its `companion.json`. A player has at most one pack per companion `id`. |
| `shown_packs` | The one pack a player shows their friends, if any. |
| `pack_reports` | Who reported which pack, why and when. |
| `pack_removals` | Packs an administrator removed, by SHA-256, so the same pack cannot come back. |
| `pack_uploads` | When each player uploaded, for the rate limit; rows older than a day are dropped. |

Packs belong to their uploader: two players who upload the same pictures
store two rows with the same hash. Deleting a pack or the account deletes
its bytes. The data download lists a player's packs without their
pictures, and their reports.

## Visibility

Each pack is visible to its `owner` only, or to the owner's `friends`. At
most one pack is visible to friends: it is the companion they see.
Marking another pack `friends` makes the earlier one `owner` again.
Group mates and other players see nothing; a public visibility is not
offered.

A friend sees the pack while the friendship lasts, while the owner keeps
the pack `friends`, and until the friend reports it. Ending the
friendship, `owner`, deleting the pack and removing it all take it away
at once: the server stops listing and serving it. A copy a friend's client
already holds stays on that device until its cache drops it; clients
drop a cached pack the friend list no longer names.

## Upload

`POST /api/packs/{user}` with the pack zip as the body, as the owner.
Uploading is the player's choice per pack; clients never upload on their
own.

The server reads the zip under the same rules as the app (AnkiQuest-Art
`docs/packs.md`, "What an app accepts"):

- The body is at most 32 MiB, read with an explicit limit before
  anything is decoded; the zip has at most 128 entries, no entry larger
  than 8 MiB once read and at most 64 MiB read in total, counting the
  bytes actually inflated, not the headers.
- Only stored and deflated entries. Entry names are `companion.json` or
  lower-case letters, digits and underscores ending in `.png` or `.webp`:
  no folders, `..`, absolute paths, backslashes or names twice. Nothing
  is ever written to disk under an entry's name.
- `companion.json` is at most 64 KiB of UTF-8 JSON in format 1. `id`
  follows the pack rule and is not an official companion's (every id in
  `companion_preferences`, planned ones and `none` included). `name` has
  1 to 40 characters, `notice` at most 16 KiB; `version` is positive.
- Every name in `poses` is a pose, extension, picture or icon of
  `tests/fixtures/companion_poses.json`, every core pose is there, every
  named file is in the zip and every picture is named.

The server is stricter than the app where friends will read what one
player wrote:

- `licence` and `credit` are required: `licence` is an SPDX expression
  (letters, digits, `.`, `-`, `+`, `(`, `)` and single spaces, at most 64
  characters), `credit` 1 to 200 characters. No control characters in
  `name`, `licence` or `credit`, nor in `notice` other than newlines and
  tabs.
- Each picture must start with its extension's signature and is fully
  decoded by the pure-Rust PNG and WebP decoders with limits: at most
  2048 px on each side and 48 MiB of decoded pixels. An animated WebP is
  refused. SVG, GIF, JPEG and any other format are refused.
- The pictures are not kept as uploaded. Each is decoded to RGBA,
  shrunk to at most 1024 px on its longer side (432 px for a file only
  launcher icons use), never upscaled, and encoded again as lossy WebP,
  quality 88 with full-quality alpha, as Art's packs are. Nothing of the
  uploaded files but their pixels survives: no metadata, extra chunks,
  colour profiles or trailing bytes.
- `companion.json` is written again from the checked fields only, with
  each picture renamed to its `.webp` name. Unknown fields are dropped.
- The canonical zip is built like Art's: stored, sorted entries, a fixed
  time and mode, no extra fields. Its SHA-256 is the pack's address. The
  same upload always gives the same bytes.

The canonical pack is at most 16 MiB. Processing runs off the request
threads, two packs at a time at most.

| answer | when |
|---|---|
| `200` with the pack (below) | stored; a pack with the same `id` the player had is replaced, and if it was visible to friends the new one is |
| `400` `{"error", "reason"}` | a rule failed; `reason` is one of `too_large`, `too_many_entries`, `bad_entry_name`, `duplicate_entry`, `not_a_picture`, `picture_too_large`, `animated`, `bad_index`, `newer_format`, `bad_id`, `reserved_id`, `unknown_pose`, `missing_core_pose`, `missing_file`, `unused_file`, `bad_licence`, `bad_credit` |
| `401` | not the owner |
| `403` `{"reason": "removed"}` | an administrator removed this pack |
| `409` `{"reason": "quota"}` | the player would have more than 5 packs or more than 48 MiB of packs |
| `413` | the body is over 32 MiB or the canonical pack over 16 MiB |
| `429` | more than 10 uploads in the last 24 hours, refused ones included |

A pack in answers:

```json
{
  "sha256": "<64 lower-case hex digits>",
  "id": "mochi",
  "name": "Mochi",
  "version": 3,
  "licence": "CC-BY-4.0",
  "credit": "Ana",
  "notice": null,
  "size": 812345,
  "visibility": "owner",
  "uploaded_at": 1793183400000
}
```

## Requests

All need a token or session of `{user}`.

| request | answers |
|---|---|
| `GET /api/packs/{user}` | `{"packs": [...], "used": <bytes>, "quota": <bytes>, "max_packs": 5}`, the player's packs by name |
| `POST /api/packs/{user}` | upload (above) |
| `PUT /api/packs/{user}/{sha256}` `{"visibility": "friends" \| "owner"}` | the pack; `404` when the player has no such pack |
| `DELETE /api/packs/{user}/{sha256}` | `204`; `404` when the player has no such pack |
| `GET /api/packs/{user}/friends` | `{"friends": {"<friend>": <pack without visibility and uploaded_at>}}`: each friend's pack visible to friends, leaving out packs this player reported |
| `GET /api/packs/{user}/file/{sha256}` | the canonical zip, `application/zip`, `ETag` the hash, `Cache-Control: private, max-age=31536000, immutable`; `404` unless the player owns a pack with this hash or a friend shows it, and it is not removed |
| `POST /api/packs/{user}/reports/{sha256}` `{"reason": "offensive" \| "copyright" \| "other"}` | `204`; the pack is hidden for this player at once and the report is logged for administrators. `404` unless a friend shows it. Reporting twice keeps the first report |
| `DELETE /api/packs/{user}/reports/{sha256}` | `204`; withdraws the report and shows the pack again |

## Moderation

- Report: any friend who sees a pack can report it. The report hides it
  for them and writes a warning to the server log with the hash, the
  owner and the reason.
- Administrators remove a pack with `ankiquest admin pack-remove
  <sha256>`: its rows go from every player who has it, nobody sees it any
  more, and the hash is kept in `pack_removals`, so uploading the same
  pack again is `403`. `admin sql "select * from pack_reports"` lists
  reports.
- There is no automatic removal after a number of reports.

## Clients

- Upload only on the player's request, per imported pack, with a choice
  of `owner` or `friends`. Show the licence and credit before uploading.
- Read `GET /api/packs/{user}/friends` with the friend list. For a pack
  not cached, fetch `GET /api/packs/{user}/file/{sha256}`, refuse it
  unless its SHA-256 is the one asked for, then run the same pack reader
  as for an import. Keep friends' packs apart from the player's own
  companions, by hash: a friend's pack is never choosable, never shadows
  an official or imported companion and its `id` means nothing outside
  that friend.
- Cache friends' packs with limits (Android: at most 16 packs and 64 MiB,
  least recently used dropped first) and drop a pack the friend list no
  longer names.
- Fallback: when a friend shows no pack, or theirs cannot be fetched or
  read (removed, `404`, a hash mismatch, a refused pack, a name this
  client does not know), draw that friend as today: their official
  companion where the client shows one, otherwise Aki. A missing picture
  in a pack follows the pack's fallbacks as for any pack.
- Share cards and `/w/` pages the server draws keep showing the player's
  official companion.
