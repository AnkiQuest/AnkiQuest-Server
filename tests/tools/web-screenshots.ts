// Renders the website pages the README banner shows into docs/screenshots/web/.
//
// The pages come from static/ with their scripts bundled the way build.rs does;
// every API call is answered from the recorded examples in docs/api/examples,
// so no server or database is needed. Everything that changes pixels is fixed:
// Playwright's own Chromium (its version comes with the pinned playwright
// package), the fonts (only assets/fonts, through a fontconfig file, so no
// system font is used), the clock, locale, time zone, colour scheme, viewport
// and device scale. Runs on Linux give byte-identical files; CI checks that.
// `cargo run -p ankiquest-xtask -- banner` then draws the banner from them.
//
// Run: npm run screenshots:web, or npm run screenshots for these, the banner
// and the social preview.
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type {Browser, BrowserContext, Route} from "playwright";
import {chromium, fulfillAsset, page as built, root} from "../support/web.ts";

/** The player the examples are recorded for. */
const PLAYER = "hill";

/** The examples' clock: Wednesday 28 October 2026, 10:30 UTC. */
const CLOCK = new Date("2026-10-28T10:30:00Z");

/** CSS pixels of the window each page is drawn in, and the device pixel ratio. */
const VIEWPORT = {width: 760, height: 1100};
const SCALE = 2;

/**
 * The only fonts the browser sees: Roboto for text (the banner's font), DejaVu
 * from the pinned dejavu-fonts-ttf package for symbols such as ↗ and ♜ and for
 * monospace. assets/fonts/DejaVuSans*.ttf are subsets for the share cards.
 */
const DEJAVU = path.join(root, "node_modules", "dejavu-fonts-ttf", "ttf");
const FONT_FILES = [
  path.join(root, "assets", "fonts", "Roboto-Regular.ttf"),
  path.join(root, "assets", "fonts", "Roboto-Bold.ttf"),
  ...["DejaVuSans.ttf", "DejaVuSans-Bold.ttf", "DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"].map(name => path.join(DEJAVU, name)),
];

/**
 * Noto Color Emoji for the trophies and medals, downloaded once from a fixed
 * release and checked by hash; it is too large to commit or to ship in npm.
 */
const EMOJI = {
  url: "https://raw.githubusercontent.com/googlefonts/noto-emoji/v2.051/fonts/NotoColorEmoji.ttf",
  sha256: "72a635cb3d2f3524c51620cdde406b217204e8a6a06c6a096ff8ed4b5fd6e27b",
  file: path.join(root, "node_modules", ".cache", "ankiquest-screenshots", "NotoColorEmoji-v2.051.ttf"),
};

/** What each generic CSS family resolves to, in order of preference. */
const SANS = ["Roboto", "DejaVu Sans", "Noto Color Emoji"];
const MONO = ["DejaVu Sans Mono", "DejaVu Sans"];
const GENERICS: Record<string, string[]> = {
  "system-ui": SANS,
  "ui-sans-serif": SANS,
  "sans-serif": SANS,
  sans: SANS,
  serif: SANS,
  monospace: MONO,
  "ui-monospace": MONO,
};

/** Chromium switches that keep text and colour the same on every machine. */
const ARGS = ["--font-render-hinting=none", "--disable-lcd-text", "--force-color-profile=srgb", "--disable-gpu"];

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

/**
 * A fontconfig file in `dir` that knows only FONT_FILES and the emoji font, with grayscale
 * antialiasing and no hinting, so the text does not depend on the system.
 */
/** The emoji font's path, downloading it first if the cache does not hold it. */
async function emojiFont(): Promise<string> {
  const digest = (bytes: Buffer) => crypto.createHash("sha256").update(bytes).digest("hex");
  if (fs.existsSync(EMOJI.file) && digest(fs.readFileSync(EMOJI.file)) === EMOJI.sha256) {
    return EMOJI.file;
  }

  // Download, check, then move into place, so a broken download is never cached.
  const response = await fetch(EMOJI.url);
  if (!response.ok) {
    throw new Error(`${EMOJI.url}: ${response.status}`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  if (digest(bytes) !== EMOJI.sha256) {
    throw new Error(`${EMOJI.url}: sha256 ${digest(bytes)}, expected ${EMOJI.sha256}`);
  }
  fs.mkdirSync(path.dirname(EMOJI.file), {recursive: true});
  fs.writeFileSync(`${EMOJI.file}.part`, bytes);
  fs.renameSync(`${EMOJI.file}.part`, EMOJI.file);
  return EMOJI.file;
}

async function fontconfig(dir: string): Promise<string> {
  const fonts = path.join(dir, "fonts");
  fs.mkdirSync(fonts);
  for (const file of [...FONT_FILES, await emojiFont()]) {
    fs.copyFileSync(file, path.join(fonts, path.basename(file)));
  }

  const family = (name: string) => `<family>${name}</family>`;
  const aliases = Object.entries(GENERICS)
    .map(([generic, prefer]) => `<alias binding="strong">${family(generic)}<prefer>${prefer.map(family).join("")}</prefer></alias>`)
    .join("\n  ");
  const file = path.join(dir, "fonts.conf");
  fs.writeFileSync(file, `<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>${fonts}</dir>
  <cachedir>${path.join(dir, "cache")}</cachedir>
  ${aliases}
  <!-- Chromium asks for system-ui without a family name; it gets Roboto. -->
  <match target="pattern">
    <edit name="family" mode="append_last"><string>${SANS[0]}</string></edit>
  </match>
  <match target="font">
    <edit name="antialias" mode="assign"><bool>true</bool></edit>
    <edit name="hinting" mode="assign"><bool>false</bool></edit>
    <edit name="rgba" mode="assign"><const>none</const></edit>
  </match>
</fontconfig>
`);
  return file;
}

/** Playwright's own Chromium with the fixed fonts; Linux only, as fontconfig is. */
async function launchFixed(dir: string): Promise<Browser> {
  if (process.platform !== "linux") {
    throw new Error("the screenshots are rendered on Linux; download them from the CI screenshots job instead");
  }
  return chromium.launch({
    headless: true,
    channel: "chromium",
    args: ARGS,
    env: {...process.env, FONTCONFIG_FILE: await fontconfig(dir)},
  });
}

/** Renders every page and writes the pictures. */
async function main(): Promise<void> {
  fs.mkdirSync(OUT, {recursive: true});
  const fonts = fs.mkdtempSync(path.join(os.tmpdir(), "ankiquest-fonts-"));
  const browser = await launchFixed(fonts);
  console.log(`Chromium ${browser.version()}`);
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
    fs.rmSync(fonts, {recursive: true, force: true});
  }
  if (missing.size > 0) {
    console.warn(`no example for: ${[...missing].join(", ")}`);
  }
}

await main();
