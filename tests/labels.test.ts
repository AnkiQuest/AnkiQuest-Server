// The shared label set in .github/labels.json is valid and holds every label
// the workflows, the labeler config and Dependabot apply.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs';
import path from 'node:path';

interface Label {name: string; color: string; description: string; aliases: string[]}

const root = path.resolve(import.meta.dirname, '..');
const github = path.join(root, '.github');
const read = (file: string) => fs.readFileSync(path.join(github, file), 'utf8');
const labels = JSON.parse(read('labels.json')) as Label[];

// GitHub's limits for a label, and the labels Dependabot adds by default for
// the ecosystems in .github/dependabot.yml.
const COLOR = /^[0-9a-f]{6}$/;
const MAX_DESCRIPTION = 100;
const DEPENDABOT = ['dependencies', 'rust', 'github_actions'];

// Label names in the workflow scripts: LABEL constants and add/remove calls.
function workflowNames(): string[] {
  const names: string[] = [];
  for (const file of ['labels.yml', 'issue-labels.yml', 'conflicts.yml']) {
    for (const match of read(path.join('workflows', file)).matchAll(/(?:LABEL = |add\(|remove\()'([^']+)'/g)) {
      names.push(match[1]);
    }
  }
  return names;
}

// Area labels are the top-level keys of the labeler config.
function labelerNames(): string[] {
  return [...read('labeler.yml').matchAll(/^([A-Za-z_][^:\n]*):$/gm)].map(match => match[1]);
}

test('labels.json entries are well formed and unique ignoring case', () => {
  const seen = new Set<string>();
  for (const label of labels) {
    assert.deepEqual(Object.keys(label).sort(), ['aliases', 'color', 'description', 'name'], label.name);
    assert.match(label.color, COLOR, label.name);
    assert.ok(label.description.length <= MAX_DESCRIPTION, label.name);
    for (const name of [label.name, ...label.aliases]) {
      const key = name.toLowerCase();
      assert.ok(!seen.has(key), `${name} appears twice`);
      seen.add(key);
    }
  }
});

test('labels.json holds every label the workflows apply, spelled the same', () => {
  const names = new Set(labels.map(label => label.name));
  const used = [...workflowNames(), ...labelerNames(), ...DEPENDABOT];
  assert.ok(workflowNames().length > 0);
  for (const name of used) {
    assert.ok(names.has(name), `${name} is missing from .github/labels.json`);
  }
});
