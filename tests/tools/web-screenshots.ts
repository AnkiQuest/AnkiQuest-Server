// Renders the website pages the README banner shows into docs/screenshots/web/.
//
// The pages come from static/ with their scripts bundled the way build.rs does;
// every API call is answered from the recorded examples in docs/api/examples,
// so no server or database is needed. The clock, locale, time zone and colour
// scheme are fixed, so repeated runs give the same pictures. The pages use the
// system font (system-ui), so another system can draw slightly different text.
// `cargo run -p ankiquest-xtask -- banner` then draws the banner from them.
//
// Run: npm run screenshots:web, or npm run banner for the screenshots and the banner.
import fs from "node:fs";
import path from "node:path";
import type {BrowserContext, Route} from "playwright";
import {fulfillAsset, launch, page as built, root} from "../support/web.ts";

/** The player the examples are recorded for. */
const PLAYER = "hill";

/** The examples' clock: Wednesday 28 October 2026, 10:30 UTC. */
const CLOCK = new Date("2026-10-28T10:30:00Z");

/** CSS pixels of the window each page is drawn in, and the device pixel ratio. */
const VIEWPORT = {width: 760, height: 1100};
const SCALE = 2;

const EXAMPLES = path.join(root, "docs", "api", "examples");
const STATIC = path.join(root, "static");
const OUT = path.join(root, "docs", "screenshots", "web");

/** A page to render: its file in static/, the path it is served at, and the output name. */
interface Shot {
  html: string;
  url: string;
  out: string;
}

/** The banner's pages, in the order the banner draws them. */
const SHOTS: Shot[] = [
  {html: "index", url: "/", out: "leaderboard.png"},
  {html: "personal", url: "/today", out: "today.png"},
  {html: "personal", url: "/friends", out: "friends.png"},
  {html: "community", url: "/community", out: "community.png"},
];

/** Answers for endpoints without a recorded example; the pages need only their shape. */
const SYNTHETIC: Record<string, unknown> = {
  "/auth/status": {private_site: false, authenticated: true, registration: true, member: {user: PLAYER}},
  "/api/avatars": [],
  "/api/winners": {},
  [`/api/leaderboard/hidden/${PLAYER}`]: [],
};

/** Content types of the files served from static/. */
const TYPES: Record<string, string> = {
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".webp": "image/webp",
  ".css": "text/css",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
};

/** The example file for an API path, such as /api/profile/hill to profile.json. */
function example(url: URL): string | undefined {
  const segments = url.pathname.split("/").slice(2);
  if (segments.at(-1) === PLAYER) {
    segments.pop();
  }
  let name = segments.join("_").replaceAll("-", "_");

  // Variants recorded under their own names.
  if (name === "leaderboard" && url.searchParams.get("period") === "day") {
    name = "leaderboard_day";
  }
  if (name === "profile" && !url.pathname.endsWith(`/${PLAYER}`)) {
    name = "player_profile";
  }
  if (name.startsWith("community_")) {
    name = name.slice("community_".length);
  }
  if (name === "sign_in") {
    name = "sign_in_methods";
  }

  const file = path.join(EXAMPLES, `${name}.json`);
  return fs.existsSync(file) ? file : undefined;
}

/** A file under static/ for a request path, if there is one. */
function staticFile(pathname: string): string | undefined {
  const file = path.join(STATIC, decodeURIComponent(pathname));
  if (!file.startsWith(STATIC + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
    return undefined;
  }
  return file;
}

/** Answers every request the page makes; returns the paths it could not answer. */
async function serve(context: BrowserContext, missing: Set<string>): Promise<void> {
  await context.route("**/*", async (route: Route) => {
    const url = new URL(route.request().url());
    const shot = SHOTS.find(candidate => candidate.url === url.pathname);

    // Pages, built assets and recorded answers, in that order.
    if (shot && route.request().resourceType() === "document") {
      return route.fulfill({contentType: "text/html", body: built(shot.html)});
    }
    if (await fulfillAsset(route, url.pathname)) {
      return;
    }
    if (url.pathname in SYNTHETIC) {
      return route.fulfill({json: SYNTHETIC[url.pathname]});
    }
    const recorded = url.pathname.startsWith("/api/") ? example(url) : undefined;
    if (recorded) {
      return route.fulfill({contentType: "application/json", body: fs.readFileSync(recorded)});
    }
    const file = staticFile(url.pathname);
    if (file) {
      return route.fulfill({contentType: TYPES[path.extname(file)] ?? "application/octet-stream", body: fs.readFileSync(file)});
    }

    missing.add(url.pathname + url.search);
    return route.fulfill({status: 404, json: {error: "no example"}});
  });
}

/** Renders every page and writes the pictures. */
async function main(): Promise<void> {
  fs.mkdirSync(OUT, {recursive: true});
  const browser = await launch();
  const missing = new Set<string>();
  try {
    for (const shot of SHOTS) {
      const context = await browser.newContext({
        viewport: VIEWPORT,
        deviceScaleFactor: SCALE,
        locale: "en-US",
        timezoneId: "UTC",
        colorScheme: "light",
        reducedMotion: "reduce",
      });
      await context.clock.setFixedTime(CLOCK);
      await serve(context, missing);
      const page = await context.newPage();
      const errors: string[] = [];
      page.on("pageerror", error => errors.push(error.message));
      await page.goto(`https://ankiquest.test${shot.url}`, {waitUntil: "networkidle"});
      await page.evaluate(() => document.fonts.ready);
      if (errors.length > 0) {
        throw new Error(`${shot.url}: ${errors.join("; ")}`);
      }
      const file = path.join(OUT, shot.out);
      await page.screenshot({path: file, animations: "disabled", caret: "hide"});
      console.log(path.relative(root, file));
      await context.close();
    }
  } finally {
    await browser.close();
  }
  if (missing.size > 0) {
    console.warn(`no example for: ${[...missing].join(", ")}`);
  }
}

await main();
