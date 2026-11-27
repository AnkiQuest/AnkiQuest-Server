// The website's heatmap colours by the same rule as both clients: the levels
// tests/fixtures/review_heatmap.json lists as intensity_cases.
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {heatmapIntensity} from '../web/common.ts';

interface IntensityCase { daily_average: number; counts: number[]; levels: number[] }

const fixture = JSON.parse(readFileSync(new URL('./fixtures/review_heatmap.json', import.meta.url), 'utf8'));

test('every intensity case of the shared fixture', () => {
  for (const entry of fixture.intensity_cases as IntensityCase[]) {
    entry.counts.forEach((count, index) => {
      assert.equal(
        heatmapIntensity(count, entry.daily_average),
        entry.levels[index],
        `count ${count} at a daily average of ${entry.daily_average}`,
      );
    });
  }
});

test('a day without reviews never colours, whatever the average', () => {
  assert.equal(heatmapIntensity(0, 0), 0);
  assert.equal(heatmapIntensity(0, 10), 0);
});

test('an average under one review counts the first review as the average', () => {
  // The rule floors the average at one, so one review is already level 2 on a quiet window.
  assert.equal(heatmapIntensity(1, 0.2), 2);
});
