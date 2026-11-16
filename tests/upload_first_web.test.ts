// Boards say why they show no one else: sign in, or study and sync once.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import type {Browser} from 'playwright';
import {fulfillAsset, launch, page as built} from './support/web.ts';

const LOCK = 'X-Ankiquest-Locked';

async function visit(browser: Browser, path: string, member: string | null, lock: string | null) {
  const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 850}});
  const headers = lock ? {[LOCK]: lock, 'Access-Control-Expose-Headers': LOCK} : undefined;
  await context.route('**/*', async route => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === '/week' || pathname === '/records') return route.fulfill({contentType: 'text/html', body: built('index')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname === '/auth/status') {
      return route.fulfill({json: {private_site: false, authenticated: !!member, registration: false, member: member && {user: member}}});
    }
    if (pathname === '/api/leaderboard' || pathname === '/api/records') return route.fulfill({json: [], headers});
    if (pathname === `/api/social/${member}`) return route.fulfill({json: {groups: [], friends: []}});
    if (pathname === '/api/week') return route.fulfill({json: {ends_at: Date.now() + 86400000, timezone: 'UTC'}});
    if (pathname === '/api/winners') return route.fulfill({json: {}});
    if (pathname === '/manifest.webmanifest') return route.fulfill({json: {name: 'AnkiQuest'}});
    if (pathname === '/icon.svg') return route.fulfill({contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg"/>'});
    return route.fulfill({status: 404, body: pathname});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://ankiquest.test' + path);
  await page.locator('.empty').first().waitFor();
  const empty = await page.locator('.empty').first().textContent();
  await context.close();
  return {empty, errors};
}

test('an empty board says to sign in, or to study and sync once, when the server says so', async () => {
  const browser = await launch();
  try {
    for (const path of ['/week', '/records']) {
      assert.deepEqual(await visit(browser, path, 'alice', 'upload_first'),
        {empty: 'Study and sync once to see how others are doing.', errors: []}, path);
      assert.deepEqual(await visit(browser, path, null, 'sign_in'),
        {empty: 'Sign in to see how others are doing.', errors: []}, path);
    }
    assert.deepEqual(await visit(browser, '/week', 'alice', null),
      {empty: 'Nobody has uploaded reviews yet.', errors: []});
  } finally {
    await browser.close();
  }
});
