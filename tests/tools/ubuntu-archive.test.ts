// Exercise the CI mirror selector with real HTTP redirects and curl, without sudo
// changing the machine. CI runs this on Ubuntu before installing browser packages.
import assert from "node:assert/strict";
import {spawn} from "node:child_process";
import {mkdtemp, readFile, rm, writeFile} from "node:fs/promises";
import {createServer} from "node:http";
import {tmpdir} from "node:os";
import path from "node:path";
import {test, type TestContext} from "node:test";

const selector = path.resolve(import.meta.dirname, "../../.github/scripts/ubuntu-archive.sh");
const HTTP_OK = 200;
const HTTP_MOVED = 301;
const HTTP_UNAVAILABLE = 503;
const SOURCES = "Types: deb\nURIs: http://security.ubuntu.com/ubuntu/\nSuites: noble-security\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n";

async function select(mirrors: string[], context: TestContext) {
  const directory = await mkdtemp(path.join(tmpdir(), "ankiquest-mirror-"));
  context.after(() => rm(directory, {recursive: true, force: true}));
  await writeFile(path.join(directory, "ubuntu.sources"), SOURCES);
  // Capture selection and apt verification instead of writing /etc or installing packages.
  await writeFile(path.join(directory, "sudo"), `#!/usr/bin/env bash
set -euo pipefail
case "$1" in
    tee) cat > "$RUNNER_TEMP/selected" ;;
    apt-get) printf '%s\\n' "$*" >> "$RUNNER_TEMP/apt" ;;
    sed) args=("\${@:2}"); args[-1]="$RUNNER_TEMP/ubuntu.sources"; sed "\${args[@]}" ;;
    *) exit 99 ;;
esac
`, {mode: 0o755});
  const child = spawn("bash", [selector, ...mirrors], {
    env: {...process.env, PATH: `${directory}${path.delimiter}${process.env.PATH}`, RUNNER_TEMP: directory},
    stdio: ["ignore", "ignore", "pipe"],
  });
  let errors = "";
  child.stderr.setEncoding("utf8").on("data", data => { errors += data; });
  const status = await new Promise<number | null>((resolve, reject) => {
    child.once("error", reject);
    child.once("close", resolve);
  });
  return {status, errors, directory};
}

test("the mirror probe follows redirects before selecting an archive", async context => {
  const requests: string[] = [];
  const server = createServer((request, response) => {
    const url = request.url ?? "";
    requests.push(url);
    if (url.startsWith("/redirect/")) {
      response.writeHead(HTTP_MOVED, {Location: "/unavailable"}).end();
    } else if (url.startsWith("/healthy/") || (url.startsWith("/no-security/") && !url.endsWith("-security/InRelease"))) {
      response.writeHead(HTTP_OK).end("release data");
    } else {
      response.writeHead(HTTP_UNAVAILABLE).end("unavailable mirror");
    }
  });
  await new Promise<void>(resolve => { server.listen(0, "127.0.0.1", resolve); });
  try {
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    const base = `http://127.0.0.1:${address.port}`;

    const fallback = await select([`${base}/redirect`, `${base}/no-security`, `${base}/healthy`], context);
    assert.equal(fallback.status, 0, fallback.errors);
    assert.equal(await readFile(path.join(fallback.directory, "selected"), "utf8"), `${base}/healthy/\n`);
    assert.ok(requests.includes("/unavailable"), "the redirect destination must be checked");
    assert.match(await readFile(path.join(fallback.directory, "apt"), "utf8"), /update.*Error-Mode=any/);
    assert.equal(await readFile(path.join(fallback.directory, "ubuntu.sources"), "utf8"),
      SOURCES.replace("http://security.ubuntu.com/ubuntu/", "mirror+file:/etc/apt/apt-mirrors.txt"));

    const unavailable = await select([`${base}/redirect`], context);
    assert.equal(unavailable.status, 1);
    await assert.rejects(readFile(path.join(unavailable.directory, "selected")), {code: "ENOENT"});
    await assert.rejects(readFile(path.join(unavailable.directory, "apt")), {code: "ENOENT"});
  } finally {
    await new Promise<void>((resolve, reject) => { server.close(error => { if (error) { reject(error); } else { resolve(); } }); });
  }
});
