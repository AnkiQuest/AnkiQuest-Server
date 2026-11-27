// streak_moods.json names a companion pose for each mood; both clients draw it with the
// chosen companion's art. These checks keep those names in the staged release's
// art/poses.json (tests/tools/stage-art.mjs), so a client can always fall back to a core pose.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {test} from "node:test";

const fixtures = path.resolve(import.meta.dirname, "fixtures");
const art = path.resolve(import.meta.dirname, "..", "art");

type Poses = {core: string[]; extensions: Record<string, string>; pictures: Record<string, string>};
type Moods = {moods: {name: string; expected: {mood: string; pose: string}}[]};

const read = <T>(name: string): T => JSON.parse(fs.readFileSync(path.join(fixtures, name), "utf8")) as T;
const poses: Poses = JSON.parse(fs.readFileSync(path.join(art, "poses.json"), "utf8"));
const moods = read<Moods>("streak_moods.json");

/** The pose a companion without art for `pose` ends at: its core pose, or null if the chain breaks. */
function core(pose: string): string | null {
  const seen = new Set<string>();
  let current = pose;
  while (!poses.core.includes(current)) {
    // A mood may name a picture; pictures fall back like extensions
    const next = poses.extensions[current] ?? poses.pictures[current];
    if (next === undefined || seen.has(current)) {
      return null;
    }
    seen.add(current);
    current = next;
  }
  return current;
}

test("every streak mood names a companion pose or picture", () => {
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
  away_run: {name: string; heatmap: {state: string}[] | null; studied: boolean; expected: number}[];
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
  // Today's own cell is not before today. A run that reaches the start of the heatmap has no
  // study day in it: a player who studied before it is away as long as the heatmap, and one
  // who never did (a new player) is not away at all.
  for (const {name, heatmap, studied, expected} of read<Away>("streak_moods.json").away_run) {
    const before = (heatmap ?? []).slice(0, -1);
    const last = before.findLastIndex((day) => day.state !== NONE);
    const away = last < 0 ? (studied ? before.length : 0) : before.length - 1 - last;
    assert.equal(away, expected, name);
  }
});

test("a freeze in effect shows the freeze picture, falling back to frozen", () => {
  const shows = new Map(moods.moods.map(({expected}) => [expected.mood, expected.pose]));
  assert.equal(shows.get("freeze"), "freeze");
  assert.equal(core("freeze"), "frozen");
});

type FreezeUsed = {
  freeze_used: {cases: {name: string; frozen_days: number; opened_today: boolean; expected: boolean}[]};
};

test("freeze_used shows at the first open after a used freeze", () => {
  // A picture, not a mood: only the first open of a day after a frozen day shows it.
  const {freeze_used: used} = read<FreezeUsed>("streak_moods.json");
  assert.ok(!moods.moods.some(({expected}) => expected.mood === "freeze_used"), "freeze_used is not a mood");
  assert.ok("freeze_used" in poses.pictures, "freeze_used is a picture");
  for (const {name, frozen_days: frozen, opened_today: opened, expected} of used.cases) {
    assert.equal(frozen > 0 && !opened, expected, name);
  }
});

type Night = {
  night: {
    default_usual: string;
    recent_days: number;
    min_days: number;
    usual_cases: {name: string; rollover_hour: number; first_reviews: string[]; expected: string}[];
  };
  moods: {
    name: string;
    today: {state: string; streak: number; goal_reached: boolean};
    night: {now: string; rollover_hour: number; first_reviews: string[]; yesterday: string};
    expected: {mood: string};
  }[];
};

const DAY_MINUTES = 24 * 60;
const HOUR_MINUTES = 60;

/** Minutes after the study day's rollover for a local HH:MM time. */
function afterRollover(time: string, rolloverHour: number): number {
  const [hours, minutes] = time.split(":").map(Number);
  return (((hours * HOUR_MINUTES + minutes - rolloverHour * HOUR_MINUTES) % DAY_MINUTES) + DAY_MINUTES) % DAY_MINUTES;
}

/** The usual first study time, in minutes after rollover, as the fixture defines it. */
function usual(night: Night["night"], firstReviews: string[], rolloverHour: number): number {
  const recent = firstReviews.slice(-night.recent_days).map((time) => afterRollover(time, rolloverHour));
  if (recent.length < night.min_days) {
    return afterRollover(night.default_usual, rolloverHour);
  }
  recent.sort((a, b) => a - b);
  return recent[Math.floor((recent.length - 1) / 2)];
}

test("the usual first study time follows the night rule", () => {
  const {night} = read<Night>("streak_moods.json");
  for (const {name, rollover_hour: rollover, first_reviews: first, expected} of night.usual_cases) {
    assert.equal(usual(night, first, rollover), afterRollover(expected, rollover), name);
  }
});

test("a streak sleeps only in the night window after a counted day", () => {
  // With a streak, asleep means the night rule applied: pending, before the usual time, comfortable.
  const fixture = read<Night>("streak_moods.json");
  for (const {name, today, night, expected} of fixture.moods) {
    const window =
      today.state === "pending" &&
      afterRollover(night.now, night.rollover_hour) < usual(fixture.night, night.first_reviews, night.rollover_hour);
    const comfortable = today.streak > 0 && night.yesterday === "counted";
    const sleeps = window && comfortable && !today.goal_reached;
    assert.equal(sleeps, today.streak > 0 && expected.mood === "asleep", name);
  }
});
