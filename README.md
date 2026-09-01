# Best Logs (Rust)

[![Lint](https://github.com/Julia-Roman/bestlogs-rs/actions/workflows/lint.yml/badge.svg)](https://github.com/Julia-Roman/bestlogs-rs/actions/workflows/lint.yml)
[![Build](https://github.com/Julia-Roman/bestlogs-rs/actions/workflows/build.yml/badge.svg)](https://github.com/Julia-Roman/bestlogs-rs/actions/workflows/build.yml)

A Rust + SvelteKit rewrite of [best-logs](https://github.com/ZonianMidian/best-logs): aggregates Twitch chat-log
availability across [justlog](https://github.com/gempir/justlog)/[rustlog](https://github.com/boring-nick/rustlog)
instances and [recent-messages2](https://github.com/robotty/recent-messages2) instances, ranks them, and serves
lookup/redirect/mirror endpoints plus a small static site — all from a single self-contained binary.

## Development

With [Nix](https://nixos.org) (flakes enabled), `nix develop` drops you into a shell with the exact Rust and Node
toolchains this project builds against — nothing to install manually:

```sh
nix develop

cd frontend && npm install && npm run build    # rebuild after frontend changes
cargo run                                      # backend, from the repo root
```

Without Nix, install Rust and Node yourself and run the same two commands.

`cargo run` picks up a `config.json` in the working directory if present (see `example_config.json` for the
schema), otherwise falls back to the built-in defaults. The Rust binary embeds `frontend/build` at compile time
(via `rust-embed`), so the frontend needs rebuilding before `cargo build` picks up frontend changes.

For frontend-only iteration, `npm run dev` inside `frontend/` proxies API paths to a `cargo run` instance on
`localhost:2028` (see `frontend/vite.config.ts`).

### Linting

```sh
cd frontend && npm run lint    # eslint + prettier --check
cd frontend && npm run format  # prettier --write
cd frontend && npm run check   # svelte-check

cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

Both are enforced in CI (`.github/workflows/lint.yml`); `.github/workflows/build.yml` builds the full `nix build`
artifact on every push and PR.

## Building

```sh
nix build              # ./result/bin/bestlogs-rs
nix run                # build and run in one step
```

Without Nix, build the frontend and then `cargo build --release` (see [Development](#development) above) —
just make sure `frontend/build` exists first.

## Deploying

The flake exports a NixOS module (`nixosModules.default`) that runs Best Logs as a systemd service:

```nix
{
  inputs.bestlogs-rs.url = "github:Julia-Roman/bestlogs-rs";

  outputs = { nixpkgs, bestlogs-rs, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        bestlogs-rs.nixosModules.default
        {
          services.bestlogs-rs = {
            enable = true;
            openFirewall = true;
            settings = {
              port = 2028;
              instance.maintainer = "yourname";
              # ...see example_config.json for the full schema
            };
          };
        }
      ];
    };
  };
}
```

Anywhere else, `nix build` and copy `result/bin/bestlogs-rs` (plus a `config.json` next to it) to the target —
it's a single static-ish binary with the frontend embedded, no other runtime dependencies beyond libc.

## Credits

Original author: [ZonianMidian](https://github.com/ZonianMidian), who designed and built the original
[best-logs](https://github.com/ZonianMidian/best-logs).

## License

MIT — see [LICENSE](LICENSE).
