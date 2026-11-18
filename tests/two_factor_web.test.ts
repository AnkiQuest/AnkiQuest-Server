// Two-factor authentication on the website against the compiled server: setting it up in Settings,
// then signing in with the password and a second factor.
import assert from 'node:assert/strict';
import {createHmac} from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {test} from 'node:test';
import {launch, startServer} from './support/web.ts';

const PASSWORD = 'long enough pw';

/** RFC 6238 with the authenticator app defaults the server uses: SHA-1, 6 digits, 30 seconds. */
function totp(secret: string, at = Date.now()): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  let bits = '';
  for (const char of secret.replace(/=+$/, '')) bits += alphabet.indexOf(char).toString(2).padStart(5, '0');
  const key = Buffer.from(bits.match(/.{8}/g)!.map(byte => parseInt(byte, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(at / 30000)));
  const digest = createHmac('sha1', key).update(counter).digest();
  const offset = digest[digest.length - 1] & 0xf;
  return String((digest.readUInt32BE(offset) & 0x7fffffff) % 1_000_000).padStart(6, '0');
}

test('a player turns on two-factor in Settings and then signs in with a recovery code', async () => {
  const run = fs.mkdtempSync(path.join(os.tmpdir(), 'ankiquest-two-factor-'));
  const server = await startServer(run, {registration: true});
  const browser = await launch();
  try {
    const {base} = server;
    const signedUp = await fetch(base + '/api/accounts', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({user: 'ana', password: PASSWORD})});
    assert.equal(signedUp.status, 201);

    // Signing in with the password alone still works, and Settings offers to set it up.
    const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 844}});
    const page = await context.newPage(), errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(base + '/login?next=%2Fsettings');
    await page.locator('#signin-user').fill('ana');
    await page.locator('#signin-password').fill(PASSWORD);
    await page.locator('#signin-form button[type=submit]').click();
    await page.waitForURL('**/settings');
    await page.locator('#two-factor-begin').click();

    // The QR code loads and the key is shown; a code from it turns two-factor on.
    const secret = (await page.locator('#two-factor code.device-token').textContent())!.trim();
    assert.match(secret, /^[A-Z2-7]{32}$/);
    await page.waitForFunction(() => (document.getElementById('two-factor-qr') as HTMLImageElement | null)?.naturalWidth! > 0);
    assert.match((await page.locator('#two-factor a.button-link').getAttribute('href'))!, /^otpauth:\/\/totp\/AnkiQuest:ana\?secret=/);
    await page.locator('#two-factor-form [name=password]').fill(PASSWORD);
    await page.locator('#two-factor-form [name=code]').fill(totp(secret));
    await page.locator('#two-factor-form button[type=submit]').click();
    await page.locator('.recovery-codes li').first().waitFor();
    const codes = await page.locator('.recovery-codes code').allTextContents();
    assert.equal(codes.length, 10);
    await page.locator('#two-factor-saved').click();
    assert.match((await page.locator('#two-factor').textContent())!, /Two-factor authentication is on\. 10 recovery codes left\./);
    assert.deepEqual(errors, []);
    await context.close();

    // A new browser: the password is right, so the form asks for the code and keeps the password.
    const later = await browser.newContext({locale: 'en-US'});
    const login = await later.newPage();
    await login.goto(base + '/login?next=%2Fsettings');
    await login.locator('#signin-user').fill('ana');
    await login.locator('#signin-password').fill(PASSWORD);
    await login.locator('#signin-form button[type=submit]').click();
    await login.locator('#signin-status').filter({hasText: 'Enter the code from your authenticator app.'}).waitFor();
    assert.equal(await login.locator('#signin-code').isVisible(), true);
    await login.locator('#signin-code').fill('000000');
    await login.locator('#signin-form button[type=submit]').click();
    await login.locator('#signin-status').filter({hasText: 'That code is not right.'}).waitFor();
    await login.locator('#signin-code').fill(codes[0].toUpperCase());
    await login.locator('#signin-form button[type=submit]').click();
    await login.waitForURL('**/settings');
    await login.locator('#two-factor').filter({hasText: '9 recovery codes left.'}).waitFor();
    await later.close();
  } finally {
    await browser.close();
    await server.stop();
    fs.rmSync(run, {recursive: true, force: true});
  }
});

test('a two-factor answer that arrives after the account changed is not shown', async () => {
  const run = fs.mkdtempSync(path.join(os.tmpdir(), 'ankiquest-two-factor-switch-'));
  const server = await startServer(run, {registration: true});
  const browser = await launch();
  try {
    const {base} = server;
    const json = {method: 'POST', headers: {'Content-Type': 'application/json'}};
    for (const user of ['ana', 'bob']) {
      const signedUp = await fetch(base + '/api/accounts', {...json, body: JSON.stringify({user, password: PASSWORD})});
      assert.equal(signedUp.status, 201);
    }
    const issued = await fetch(base + '/api/accounts/tokens', {...json, body: JSON.stringify({user: 'bob', password: PASSWORD})});
    assert.equal(issued.status, 200, await issued.clone().text());
    const {token} = await issued.json() as {token: string};

    // Ana opens Settings with her password.
    const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 844}});
    const page = await context.newPage(), errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(base + '/login?next=%2Fsettings');
    await page.locator('#signin-user').fill('ana');
    await page.locator('#signin-password').fill(PASSWORD);
    await page.locator('#signin-form button[type=submit]').click();
    await page.waitForURL('**/settings');
    await page.locator('#two-factor-begin').waitFor();

    // Hold the body of Ana's set-up answer after its headers arrive.
    await page.evaluate(() => {
      const held = window as unknown as {release: () => void; waiting: boolean};
      const gate = new Promise<void>(resolve => { held.release = resolve; });
      const json = Response.prototype.json;
      Response.prototype.json = async function (this: Response) {
        if (this.url.includes('/api/two-factor/ana')) { held.waiting = true; await gate; }
        return json.call(this);
      };
    });
    await page.locator('#two-factor-begin').click();
    await page.waitForFunction(() => (window as unknown as {waiting?: boolean}).waiting === true);

    // Meanwhile Ana disconnects and Bob connects with his device token.
    await page.locator('#personal-disconnect').click();
    await page.locator('#personal-gate:visible').waitFor();
    await page.locator('#personal-connect [name=user]').fill('bob');
    await page.locator('#personal-connect [name=token]').fill(token);
    await page.locator('#personal-connect button').click();
    await page.locator('.personal-lead h2').filter({hasText: 'bob'}).waitFor();
    await page.locator('#two-factor-begin').waitFor();

    // Ana's key arrives late and is dropped: Bob's page still offers to set up his own.
    await page.evaluate(() => (window as unknown as {release: () => void}).release());
    await page.waitForTimeout(500);
    assert.equal(await page.locator('#two-factor .device-token').count(), 0);
    assert.equal(await page.locator('#two-factor-begin').count(), 1);
    assert.deepEqual(errors, []);
    await context.close();
  } finally {
    await browser.close();
    await server.stop();
    fs.rmSync(run, {recursive: true, force: true});
  }
});
