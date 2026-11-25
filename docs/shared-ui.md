# Shared UI package: design note

Status: proposal, nothing implemented. It asks whether the heatmap, moment and share cards,
companion view and stats should be one TypeScript package used by the server's web pages, the
desktop add-on's web views and Android WebViews, the way `crates/rules` shares the rules
(`docs/shared-rules.md`).

## What each client has today

| Piece | Server web pages | Desktop add-on | Android app |
| --- | --- | --- | --- |
| Heatmap | `heat()` in `web/pages/index.ts`: last 26 weeks, server verdict colours, level relative to the busiest day | `heatmap_ui.py`: HTML and an inline script in the deck list, a year at a time with forecast, level from `heatmap_intensity` in the rules module (AnkiQuest/AnkiQuest-Desktop#433) | `heatmap/ReviewHeatmap*.kt`: custom View (about 1,200 lines), same rules as the add-on |
| Today panel, stats | `web/pages/index.ts`, `web/personal.ts` (TypeScript, DOM strings) | `board.py`: HTML strings built in Python, under the deck list | native views |
| Moment cards | `web/pages/conquered.ts` (one kind, public page) | `moments_ui.py`: Qt widgets | `AnkiquestMomentCard.kt` (about 980 lines) |
| Share pictures | `src/cards.rs`: SVG rasterised with resvg on the server | `share_picture.py`: QPainter | `AnkiquestShare.kt`: Android canvas |
| Companion view | pose pictures on the pages | `companion_ui.py`: Qt widgets; poses in HTML on the Today panel | `AnkiquestCompanion.kt`, pickers |
| Text | `ankiquest-i18n` catalogs through `crates/i18n-wasm` | its own catalogs (`language.py`) | Android string resources |
| Build | esbuild from `build.rs`, embedded in the server | none: strings in Python | Gradle |

The same piece exists three times in three toolkits. Only the rules behind them are shared: by the
fixtures, and now by `crates/rules`. The website's heatmap even uses a different level rule (relative
to the busiest day) from the clients' `intensity_cases`.

Only two of the three clients draw in a browser engine. The add-on's deck list and Today panel are
Qt WebEngine pages, but its windows are Qt widgets. The Android app has no WebView in its AnkiQuest
code, and its widgets and notifications cannot host one.

## What would move

A package `ui/` in AnkiQuest-Server (`@ankiquest/ui`, private), next to `web/`:

- **Heatmap**: one year or the last N weeks, verdict colours, forecast, kept days, the day detail.
  Levels from `heatmap_intensity` (the rules module), so the website takes the clients' rule.
- **Stats tiles**: streak, longest streak, days studied, reviews per day.
- **Moment card**: the kinds the fixtures list (milestone, level up, place, deck conquered), with
  the companion's pose.
- **Companion view**: pose for today's mood (`streak_moods.json`) with the fallbacks from the pinned
  `poses.json`.
- **Design tokens**: colours, radii and type sizes as CSS custom properties and as JSON, so a
  native client can read the same values.

Each component is a function from plain data (the API's JSON shapes, already pinned by
`docs/api/examples`) and texts to a DOM subtree, plus a stylesheet scoped under one class. No
framework: the pages are plain TypeScript today, and a framework runtime would be the largest part
of the add-on's inline payload. Texts come in as arguments, so each client keeps its catalogs; the
server passes its i18n lookups.

Share pictures stay where they are drawn now. A picture must be a PNG the same on every client and
for link previews; the server's resvg cards already are, and drawing them from DOM would need a
browser to rasterise. The clients could later fetch the server's card instead of drawing their own,
which is a separate question.

## Packaging and versions

- Source in `ui/`, TypeScript with the server's `tsconfig` settings, tested with `node --test`.
- `build.rs` bundles it for the server's pages, as it does `web/`.
- A release job builds `ankiquest-ui.js` (one IIFE, no imports) and `ankiquest-ui.css` and publishes
  them with the rules module, in the same `rules-<N>` release and pin file, since components call the
  module. Consumers pin by SHA-256 as they pin Art.
- The bundle targets the Qt WebEngine of the oldest Anki the add-on supports (23.10): no syntax or
  CSS that its Chromium lacks. The add-on's CI already has an Anki 23.10 job; the component tests run
  there in WebEngine, as `qt_heatmap_rules.py` does for the module.
- The add-on inlines the bundle into its pages, as it inlines the module, so pages work offline and
  need no web export.
- A component's data and text arguments are its interface: a change that breaks them is a new major
  version of the package, recorded in the release notes, and the pin update in each client is the
  migration.

## Android

| | WebView components | Native views (today) |
| --- | --- | --- |
| Code | one implementation with the website and add-on | a Kotlin copy per piece |
| Look | web styling inside a Material app | Material, system fonts, dynamic colour |
| Cost | WebView start-up and memory per screen, a JavaScript bridge for clicks | none extra |
| Reach | activities only: not widgets, notifications or the share sheet | everywhere |
| Tests | WebView tests need an emulator | Robolectric and screenshot tests already exist |

The recommendation is that Android stays native and shares the rules (`crates/rules` through
UniFFI), the fixtures and the design tokens, not the views. A WebView would cover only some screens,
so the Kotlin views would stay for the rest.

## Costs and risks

- **Anki's styles**: every Anki web view loads `webview.css`, which sets `box-sizing` and button
  margins on everything (`tests/qt_today_layout.py` in the add-on catches one such bug). Components
  need a reset under their scope class and a layout test in Anki's WebEngine.
- **Engine age**: the bundle is held to Anki 23.10's Chromium until the add-on drops it (Anki 24.11
  is proposed in AnkiQuest/AnkiQuest-Desktop#381).
- **Three screenshot suites** (server Playwright, add-on WebEngine, Android) would change together
  on every visual change of a shared component.
- **Coupling**: a component change reaches the add-on only through a release and a pin update, as
  rules and art do. The server's own pages always use `master`.
- **Moving the add-on's HTML out of Python** changes how its pages are built and tested: data stays
  in Python, layout moves to TypeScript. `board.py` and `heatmap_ui.py` are about 1,000 lines.
- **Parity rule**: the website would change look where it adopts the clients' heatmap and cards
  (the level rule above, the forecast).

## Order, if accepted

1. Design tokens: one JSON and CSS file; the server's pages and the add-on read them, Android reads
   the JSON in a Gradle task.
2. Heatmap: server page and add-on deck list, replacing `heat()` and the script in `heatmap_ui.py`.
   The website takes the clients' level rule.
3. Stats tiles and the companion view on the add-on's Today panel and the website's profile.
4. Moment cards on the website, then in the add-on where a page shows them.

Each step is one pull request in AnkiQuest-Server and one in AnkiQuest-Desktop, each with its tests.
