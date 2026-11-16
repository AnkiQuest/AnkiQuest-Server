// AGENTS.md is shared by the three AnkiQuest repositories: everything above
// "## This repository" is copied into AnkiQuest-Desktop and AnkiQuest-Android,
// whose CI compares their copy with this file. These checks keep the split
// point the copies rely on.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {test} from "node:test";

const root = path.resolve(import.meta.dirname, "..");
const SPLIT = "## This repository";

const read = (name: string) => fs.readFileSync(path.join(root, name), "utf8");

test("CLAUDE.md only points at AGENTS.md", () => {
  assert.equal(read("CLAUDE.md").trim(), "@AGENTS.md");
});

test("AGENTS.md ends with one per-repository section", () => {
  const headings = read("AGENTS.md").split("\n").filter(line => line.startsWith("## "));
  assert.equal(headings.filter(line => line === SPLIT).length, 1, `exactly one "${SPLIT}" heading`);
  assert.equal(headings.at(-1), SPLIT, `"${SPLIT}" is the last section`);
});
