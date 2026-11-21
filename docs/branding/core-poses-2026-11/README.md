# Core pose additions

Part of [#88](https://github.com/AnkiQuest/AnkiQuest-Desktop/issues/88). This batch adds
Aki's sad, hopeful, frozen and neglected poses, Ankilope's sad, hopeful,
frozen, asleep and neglected poses, plus Anki-tan's full and chibi frozen
poses. Pose selection belongs to the companion implementation; these
batches supply art and exports.

![Existing references and new poses, on white and dark backgrounds](review.png)

The images were generated with OpenAI's built-in image generation tool
(`image_gen`). The tool did not expose its model identifier; no model name is
asserted. Three candidates were generated for each pose. The selected candidates
are Aki sad B, hopeful B, frozen A and neglected B, plus Anki-tan full C and
chibi C. Ankilope selections are sad A, hopeful B, frozen C, asleep A and
neglected C. [Exact prompts](generation.json) include all 33 candidates.
[Reusable prompts](prompts.md) split style from pose.

The references are the existing Aki masters and Shigeyuki's Anki-tan asset pack
from [#274](https://github.com/AnkiQuest/AnkiQuest-Desktop/pull/274), commit
`cd46210748d5dd7daaaf0823804e3019c1df4425`. The full pose keeps her hand-drawn
linework; the chibi keeps the emoji pack's stepped outlines and flat colours.
Both show three headband stars. Aki keeps the orange/cream palette and indigo
scarf and satchel. The owner has final approval of their appearance.

Anki-tan by Shigeyuki (https://www.patreon.com/Shigeyuki). Both new Anki-tan poses,
their exports and her portions of this comparison are derivatives licensed
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), under
[her LICENSE](../../../static/anki-tan/LICENSE). Her original files are unchanged.

All masters are 1254 x 1254 RGBA PNGs without text. The established
[alpha cleanup](../alpha-cleanup/README.md) removes pixels with alpha 1-4, plus
isolated 8-connected components whose maximum alpha is 16 or less. It does not
erode or redraw contours. [Measurements](measurements.json) record hashes, pixel
counts and bounds. The resulting poses were inspected on white and dark at
large size and 32 px.

Exports use the existing Rust tools: 1024 px WebP for Android and 256 px indexed
PNG for desktop. Export Android art into an empty staging directory, then copy
only the eleven new files; export desktop without `--replace` (since removed:
export-addon-art now overwrites a replaced pose). Existing art stays
untouched. Android parity is supplied in a linked companion-art PR.

[Desktop and Android export comparison](exports-review.png) checks the generated
PNG/WebP at the same visible size on white and dark backgrounds. Android parity:
[issue #293](https://github.com/AnkiQuest/AnkiQuest-Android/issues/293).

[Aki’s completed core additions beside the existing references](../../../static/retired/aki/review/aki-core-review.png)
show both light and dark backgrounds. [Their desktop and Android exports](../../../static/retired/aki/review/aki-core-exports.png)
were also compared at the same visible size.

The first batch shipped in #288 after the reference pack in #274 and icon
exports in #277. It includes the add-on settings credit from #287, as the
owner requested.

[Ankilope’s five new core poses beside the existing references](../../../static/retired/ankilope/review/ankilope-core-review.png)
show white and dark backgrounds, with 64 px and 32 px copies. The frozen
master uses `frozen`, distinct from a used-freeze picture. The asleep master
used the established `sleeping` export name; since #32 it is `asleep`,
like every companion's.
[Desktop and Android exports](../../../static/retired/ankilope/review/ankilope-core-exports.png) are compared at
matching sizes. Linux re-export at `0f12507` verified all five Android
files byte for byte. Four desktop files also match; the hopeful PNG uses
the canonical Linux export from `1fd6068` (SHA-256
`8bc4f948b6ab82d32e47ffbf5100f25fb73fd95e03e498bd2223f39836d04585`).
The preview sheet predates that palette-only change, whose alpha is
unchanged. Existing art is preserved. Verification:
[desktop #310](https://github.com/AnkiQuest/AnkiQuest-Desktop/pull/310#issuecomment-6894935747),
[Android #323](https://github.com/AnkiQuest/AnkiQuest-Android/pull/323#issuecomment-6894937822).
