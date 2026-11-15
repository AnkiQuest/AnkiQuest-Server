<p align="center">
<img alt="" src="docs/graphics/banner_readme.png"/>
</p>

<a href="https://github.com/AnkiQuest/AnkiQuest-Server/actions/workflows/ci.yml"><img src="https://github.com/AnkiQuest/AnkiQuest-Server/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI"/></a>
<a href="https://github.com/AnkiQuest/AnkiQuest-Server/commits/master"><img src="https://img.shields.io/github/commit-activity/m/AnkiQuest/AnkiQuest-Server" alt="commit-activity"/></a>
<a href="https://github.com/AnkiQuest/AnkiQuest-Server/graphs/contributors"><img src="https://img.shields.io/github/contributors/AnkiQuest/AnkiQuest-Server" alt="contributors"/></a>
<a href="https://github.com/AnkiQuest/AnkiQuest-Server/blob/master/LICENSE"><img src="https://img.shields.io/github/license/AnkiQuest/AnkiQuest-Server" alt="license"/></a>

# AnkiQuest-Server

The Rust server and web pages for AnkiQuest: XP, streaks, daily quests, friends and leaderboards for Anki. Clients send review history without card content. Sharing a conquered card label or a deck copy is an explicit choice.

The clients live in their own repositories:

- [AnkiQuest-Desktop](https://github.com/AnkiQuest/AnkiQuest-Desktop): the Anki desktop add-on.
- [AnkiQuest-Android](https://github.com/AnkiQuest/AnkiQuest-Android): AnkiDroid with AnkiQuest.

## Use a hosted server

Create an account on a server, such as [ankiquest.rationality-munich.com](https://ankiquest.rationality-munich.com), then sign in from a client.

## Self-host

Follow the [installation guide](docs/install.md) for a local build, NixOS with HTTPS, health checks, backups and updates, and [server configuration](docs/server.md) for settings and administration. AnkiQuest does not replace AnkiWeb or an Anki sync server.

On NixOS, add the flake input and import its module:

```nix
inputs.ankiquest.url = "github:AnkiQuest/AnkiQuest-Server";
imports = [inputs.ankiquest.nixosModules.default];
```

## Development

```sh
rustup target add wasm32-unknown-unknown
npm ci
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run typecheck
npm run test:web
```

Repository tools are in `crates/xtask` (`cargo run -p ankiquest-xtask -- <command>`), including the exporters that write companion art from `static/` into an Android or Desktop checkout. See [development](docs/server.md#development).

## API and shared fixtures

- [Client API](docs/mobile-api.md) and recorded [API examples](docs/api/examples/).
- `tests/fixtures/*.json`: rules both clients compute (streak verdicts, heatmap, card labels, deck names, companion poses). The server and both clients test against these cases.

Clients copy the examples and fixtures at a pinned commit of this repository.

## License

AGPL-3.0-only.

Anki-tan (`static/anki-tan/`) is by [Shigeyuki](https://www.patreon.com/Shigeyuki), from the [Anki-tan asset pack](https://shigeyukey.github.io/shige-addons-wiki/Anki-tan.html), licensed [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). Derivatives of her art are CC BY-SA 4.0 too.
