// streak_moods.json names a companion pose for each mood; both clients draw it with the
// chosen companion's art. These checks keep those names in companion_poses.json, so a
// client can always fall back to a core pose.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {test} from "node:test";

const fixtures = path.resolve(import.meta.dirname, "fixtures");

type Poses = {core: string[]; extensions: Record<string, string>};
type Moods = {moods: {name: string; expected: {mood: string; pose: string}}[]};

const read = <T>(name: string): T => JSON.parse(fs.readFileSync(path.join(fixtures, name), "utf8")) as T;
const poses = read<Poses>("companion_poses.json");
const moods = read<Moods>("streak_moods.json");

/** The pose a companion without art for `pose` ends at: its core pose, or null if the chain breaks. */
function core(pose: string): string | null {
  const seen = new Set<string>();
  let current = pose;
  while (!poses.core.includes(current)) {
    const next = poses.extensions[current];
    if (next === undefined || seen.has(current)) {
      return null;
    }
    seen.add(current);
    current = next;
  }
  return current;
}

test("every streak mood names a companion pose", () => {
  for (const mood of moods.moods) {
    assert.notEqual(core(mood.expected.pose), null, mood.name);
  }
});

test("every mood but fire shows one pose", () => {
  // Fire burns by heat; the other moods do not depend on the streak's length.
  const byMood = new Map<string, Set<string>>();
  for (const {expected} of moods.moods) {
    byMood.set(expected.mood, (byMood.get(expected.mood) ?? new Set()).add(expected.pose));
  }
  for (const [mood, shown] of byMood) {
    if (mood !== "fire") {
      assert.equal(shown.size, 1, mood);
    }
  }
});

test("the frozen moods fall back to the frozen pose", () => {
  for (const pose of ["frozen_2", "frozen_3"]) {
    assert.equal(core(pose), "frozen", pose);
  }
});

test("thawing and almost are moods mapped to frozen and hopeful", () => {
  // They need no picture of their own, so they are not poses (AGENTS.md, Glossary).
  const shows = new Map(moods.moods.map(({expected}) => [expected.mood, expected.pose]));
  assert.equal(shows.get("thawing"), "frozen");
  assert.equal(shows.get("almost"), "hopeful");
  for (const mood of ["thawing", "almost"]) {
    assert.ok(!poses.core.includes(mood) && !(mood in poses.extensions), `${mood} is not a pose`);
  }
});

type Stages = {
  stages: {
    starts: {stage: string; from_percent: number}[];
    cases: {time_ms: number; streak_minutes: number; expected: string}[];
  };
};

const MINUTE_MS = 60_000;
const PERCENT = 100;

test("the stage cases follow the stage starts", () => {
  // Whole-number comparison, as the fixture says, so no rounding moves a boundary.
  const {starts, cases} = read<Stages>("streak_moods.json").stages;
  assert.equal(starts[0].from_percent, 0);
  for (const {time_ms: time, streak_minutes: minutes, expected} of cases) {
    const reached = starts.filter((start) => minutes > 0 && time * PERCENT >= start.from_percent * minutes * MINUTE_MS);
    const stage = reached.at(-1)?.stage ?? starts[0].stage;
    assert.equal(stage, expected, `${time} ms of ${minutes} minutes`);
  }
});

type Away = {
  away_levels: {mood: string; from_days: number}[];
  away_run: {name: string; heatmap: {state: string}[] | null; expected: number}[];
};

const NONE = "none";

test("the away levels rise and show the neglected poses", () => {
  // neglected, abandoned and dead are levels of one mood family: each later level starts later
  // and every one ends at the neglected core pose.
  const {away_levels: levels} = read<Away>("streak_moods.json");
  const shows = new Map(moods.moods.map(({expected}) => [expected.mood, expected.pose]));
  for (const [index, level] of levels.entries()) {
    assert.ok(index === 0 || level.from_days > levels[index - 1].from_days, level.mood);
    assert.equal(core(shows.get(level.mood) ?? ""), "neglected", level.mood);
  }
});

test("the away run counts days without reviews after the last study day", () => {
  // Today's own cell is not before today; a run that reaches the start of the heatmap has no
  // study day before it, so it is a new player, not one away.
  for (const {name, heatmap, expected} of read<Away>("streak_moods.json").away_run) {
    const before = (heatmap ?? []).slice(0, -1);
    const last = before.findLastIndex((day) => day.state !== NONE);
    const away = last < 0 ? 0 : before.length - 1 - last;
    assert.equal(away, expected, name);
  }
});
