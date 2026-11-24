// Stages the AnkiQuest-Art release that ankiquest-art.json pins, for
// everything that reads the masters and the archive (build.rs, the xtask
// coverage and the tests). No repository consumes AnkiQuest-Art as a
// submodule:
//
//   node tests/tools/stage-art.mjs [repo-dir]
//
// Downloads the assets the repository reads (masters.zip, archive.zip,
// manifest.json, poses.json) from AnkiQuest-Art's release <tag>, checks
// each against the SHA-256 the pin records, and unpacks the two zips into
// <repo-dir>/art, the folder crates/art names with MASTERS. Idempotent:
// a stage whose pin matches is left alone.
//
// AnkiQuest-Sync updates the pin when AnkiQuest-Art publishes a release;
// never edit ankiquest-art.json by hand.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** The assets the repository reads, in the release and the pin. */
const ASSETS = ["masters.zip", "archive.zip", "manifest.json", "poses.json"];

/** Reads the pin in `repo`'s ankiquest-art.json. */
export function readPin(repo) {
  return JSON.parse(readFileSync(join(repo, "ankiquest-art.json"), "utf8"));
}

/** Downloads an asset of the pinned release and checks its SHA-256. */
export async function asset(pin, name) {
  const url = `https://github.com/AnkiQuest/AnkiQuest-Art/releases/download/${pin.tag}/${name}`;
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`${name}: HTTP ${response.status} from ${url}`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (digest !== pin.assets[name]) {
    throw new Error(`${name}: SHA-256 ${digest}, the pin expects ${pin.assets[name]}`);
  }
  return bytes;
}

/** Stages the pinned release into `<repo>/art`, replacing what was there. */
export async function stage(repo) {
  const pin = readPin(repo);
  for (const name of ASSETS) {
    const bytes = await asset(pin, name);
    if (name.endsWith(".zip")) {
      const archive = join(repo, `.${name}`);
      writeFileSync(archive, bytes);
      execFileSync("unzip", ["-qo", archive, "-d", join(repo, "art")], { stdio: "inherit" });
    } else {
      writeFileSync(join(repo, "art", name), bytes);
    }
  }
  console.log(`staged AnkiQuest-Art ${pin.tag} (${pin.commit.slice(0, 9)}) into ${join(repo, "art")}`);
}

// Run when called as a script, not when imported.
if (process.argv[1] && process.argv[1].endsWith("stage-art.mjs")) {
  const repo = process.argv[2] ?? ".";
  if (!existsSync(join(repo, "ankiquest-art.json"))) {
    console.error("usage: node tests/tools/stage-art.mjs [repo-dir] (ankiquest-art.json not found)");
    process.exit(2);
  }
  mkdirSync(join(repo, "art"), { recursive: true });
  await stage(repo);
}
