# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Rust (axum) + SvelteKit rewrite of [best-logs](https://github.com/ZonianMidian/best-logs): aggregates Twitch
chat-log availability across many independent [justlog](https://github.com/gempir/justlog)/[rustlog](https://github.com/boring-nick/rustlog)
instances and [recent-messages2](https://github.com/robotty/recent-messages2) instances, ranks them, and exposes
lookup/redirect/mirror/recent-messages endpoints plus a small site — all as one self-contained binary (the built
SvelteKit frontend is embedded into the Rust binary via `rust-embed`). MIT-licensed with the original author's
permission (original repo is AGPL).

**The public API (paths, query params, response shapes, status codes) must stay byte-compatible with the
original** — real third parties (bots, Chatterino-style mirror clients) depend on it. When changing backend
behavior, check whether it touches a route in `src/web/mod.rs` outside the `/meta*` namespace before assuming
you're free to reshape it.

## Commands

Nix (flakes enabled) is the primary dev/build path and gives the exact toolchain versions used in CI:

```sh
nix develop                                    # rust + node + cmake/pkg-config shell
cd frontend && npm install && npm run build    # (re)build the frontend — required before cargo build/clippy
cargo run                                      # backend, from repo root
```

Without Nix, install Rust and Node yourself and run the same two build steps.

- `cargo build`/`cargo run` embed `frontend/build` at compile time via `rust-embed` — **rebuild the frontend
  first**, or the binary serves a stale/missing site. `build.rs` registers `frontend/build` with cargo's
  change-tracking so a `cargo build` after a frontend rebuild picks it up automatically.
- `cargo run` reads `config.json` in the cwd if present (see `example_config.json` for schema/defaults); a
  missing or broken `config.json` falls back to the built-in defaults with a logged warning, never a crash.
- Frontend-only iteration: `npm run dev` in `frontend/` proxies API paths to a `cargo run` instance on
  `localhost:2028` (see `frontend/vite.config.ts` for the exact proxied paths).
- No `#[test]`s exist yet; `cargo test` runs 0 tests. Verification is done by running the server and curling/
  screenshotting it (see below), not a unit test suite.

Linting (both enforced in `.github/workflows/lint.yml`):

```sh
cd frontend && npm run lint    # eslint + prettier --check
cd frontend && npm run format  # prettier --write (fixes eslint's formatting complaints)
cd frontend && npm run check   # svelte-check

cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

Building/running the release artifact:

```sh
nix build   # ./result/bin/bestlogs-rs — the authoritative "does everything actually work" check
nix run
```

`.github/workflows/build.yml` runs `nix flake check` + `nix build` on every push/PR — this is what actually
builds frontend+backend together end to end, more so than the lint job.

**If `frontend/package-lock.json` changes**, `flake.nix`'s `npmDepsHash` goes stale and `nix build` fails with a
hash-mismatch error. Recompute it with:

```sh
nix run nixpkgs#prefetch-npm-deps -- frontend/package-lock.json
```
and paste the resulting `sha256-...` into `npmDepsHash` in `flake.nix`.

Nix flake checks/builds only see files in git's index — after adding/editing files, `git add` them (no need to
commit) before `nix build`/`nix flake check` will pick them up.

## Architecture

### Backend (`src/`)

- `main.rs` — boot order: init tracing → load config → bind the TCP listener → spawn background reload loops →
  `axum::serve`. The server starts accepting connections before the first channel-list load completes.
- `config.rs` — `Config::load()` is infallible: `example_config.json` is compiled in via `include_str!` as the
  always-available default; `config.json` in the cwd, if valid, is merged over it key-by-key at the top level.
  The one env override is `BESTLOGS_UMAMI_TOKEN`, which wins over `umamiStats.token` so the credential can be
  supplied out-of-band (systemd `EnvironmentFile`) instead of living in a checked-in/Nix-store `config.json`.
- `state.rs` — `AppState` (one `Arc<AppState>` for the process) holds the shared `reqwest::Client`, config, and
  `Caches`. Two different caching mechanisms coexist:
  - `instance_channels`/`unique_channels` (`DashMap`): each justlog/rustlog instance's channel list, refreshed by
    `reload.rs`'s background loops (full reload hourly, down-instances-only recheck every minute — an instance
    with an empty channel list is considered down and excluded from `alive_instances()`). The values are
    `logs::channels::InstanceChannels`, not bare `Vec<Channel>`: the lists are enormous (~1.6M entries across
    the configured instances, one instance alone carrying ~1M) and every lookup membership-tests *every* alive
    instance, so it answers `contains()` by binary search — the vector is kept sorted by login (the common
    lookup), with a sorted `u32` index alongside it for `id:`-style references. `/instances` therefore
    serializes channels in login order rather than the instance's own; nothing depends on that ordering.
  - `list_data`/`status_codes`/`info_data` (`moka::future::Cache`, TTL'd): per-channel/user probe results and
    ivr.fi user lookups. These use `try_get_with` so concurrent requests for the same key coalesce into one
    upstream fetch instead of each firing a duplicate; **failed** probes are deliberately never cached (only a
    genuine answer, including a genuinely-empty one, is), so a timeout/5xx self-heals on the next request rather
    than being stuck for the rest of the TTL.
- `logs/instance.rs` — the core ranking algorithm (`get_instance`/`get_logs`, ported from the original's
  `getInstance`/`getLogs`): resolves channel/user via `twitch.rs`, fans out to every alive instance concurrently
  (and, per instance, issues the `/list` day-count probe and the user-availability probe together rather than
  back to back — they're independent GETs, and serially they doubled the fan-out's cold latency),
  classifies each via the `GetLogsOutcome` enum (Down/ChannelNotFound/OptedOut/ChannelOnly/Available — an enum on
  purpose, so a caller can never observe a "status without a link" situation), and sorts by log-day count.
  **Important invariant**: the `link`/`Link` field an instance contributes is always built from the config's
  display key (`https://{key}`), never from its `alternate` host — `alternate` exists only so background
  reload/probing can hit a different backend address than the one shown to users. The mirror proxy
  (`logs/mirror.rs`) reuses this same `Link` to decide where to actually forward the proxied request.
- `logs/mirror.rs` — backs `/list`, `/channel/*`, `/channelid/*`: regex-extracts channel/user from the raw
  incoming URL, ranks via `get_instance`, rewrites the path to use resolved Twitch IDs, and proxies the request
  through.
- `logs/recent_messages.rs` — backs `/rm`, `/recent-messages`, `/api/v2/recent-messages`: queries configured
  recent-messages instances, then (unless `rm_only=true`) backfills older history from the best-ranked rustlog
  instance day-by-day, tagging backfilled lines `historical=1;` the way Chatterino expects.
- `web/` — route wiring (`mod.rs`), the public API handlers (`api.rs`), the new `/meta*` JSON endpoints that
  exist purely to feed the SvelteKit frontend (`meta.rs`, deliberately namespaced away from the public API so
  they can't collide with a real channel named e.g. `status`), and the embedded-frontend fallback handler
  (`static_files.rs`). `ratelimit.rs` is a per-IP token bucket applied with `route_layer` to the log-lookup
  routes only (`/api/*`, `/rdr/*`, `/list`, `/channel/*`, `/channelid/*`) — recent-messages is deliberately
  exempt, since Chatterino opens one request per joined channel on connect. It reads `rateLimit` from the
  config (`enabled`, `events`, `intervalSeconds`, `trustProxy`); with `trustProxy` off (the default) only the
  peer address counts, since forwarding headers are forgeable on a directly-exposed deployment. The peer
  address reaches it because `main.rs` serves via `into_make_service_with_connect_info`.
- `util.rs` — shared regexes. They spell out `[a-z0-9_]` explicitly instead of using the `regex` crate's
  Unicode-aware `\w`, to match JS's ASCII-only `\w` semantics from the original.

### Frontend (`frontend/`)

SvelteKit with `adapter-static`, `ssr = false` set repo-wide (`src/routes/+layout.ts`) — every page is fully
client-rendered and fetches its own data from the backend at runtime via `src/lib/api.ts`, so the static build
never needs a live backend and the output can be embedded byte-for-byte into the Rust binary.

- Tailwind v4, CSS-first config in `src/app.css`. Light/dark theming is **not** Tailwind's default
  `prefers-color-scheme` strategy — it's `@custom-variant dark (&:where(.dark, .dark *))` plus semantic color
  tokens (`--color-fg`, `--color-surface`, `--color-line`, `--color-accent`, etc.) redefined once for light
  (the `@theme` defaults) and once under `.dark`. Prefer adding/using a semantic token over a literal
  `white/opacity` or `black/opacity` utility, or it won't adapt between themes. The `.dark` class is applied to
  `<html>` by an inline boot script in `app.html` (reads `localStorage`, falls back to system preference,
  defaults to dark) that runs before first paint to avoid a flash of the wrong theme; `src/lib/theme.svelte.ts`
  is the runtime toggle store.
- `bits-ui` for accessible primitives actually in use (Collapsible for the mobile nav, Accordion for FAQ, Avatar
  for Contact) and `@lucide/svelte` for generic icons — lucide ships no brand/logo marks, so the X/Twitch/
  GitHub/Discord icons on the Contact page are hand-inlined SVG paths in `BrandIcon.svelte` (sourced from
  simple-icons, not lucide).
- `src/lib/meta.svelte.ts` memoizes the `/meta` fetch (version, instance list, umami config) so every page that
  needs it shares one request instead of re-fetching.
- Internal navigation links use SvelteKit's `resolve()` from `$app/paths` (required by
  `eslint-plugin-svelte`'s `svelte/no-navigation-without-resolve`) — **except** links that point at backend
  endpoints rather than SvelteKit routes (the API docs page's example links, `/rdr/*` built at runtime in
  `Navbar.svelte`), which are correctly plain strings with the lint rule explicitly disabled inline; don't
  "fix" those by wrapping them in `resolve()`, it would be wrong (those paths aren't in the SvelteKit route
  manifest).

### Deployment (`flake.nix`)

`packages.default` builds the frontend via `buildNpmPackage` and copies its output into
`rustPlatform.buildRustPackage`'s `preBuild` before compiling (so `rust-embed` has something to embed).
`nixosModules.default` exposes `services.bestlogs-rs` (`enable`, `settings` → written to a `config.json` in the
service's `WorkingDirectory` via `pkgs.writeTextDir`, `environmentFile`, `openFirewall`) for NixOS deployments.
Anything in `settings` is world-readable via the Nix store, so secrets go through `environmentFile` instead —
systemd reads it as root, which is what makes it work alongside `DynamicUser`. `aws-lc-rs`
(reqwest's rustls crypto backend) needs `cmake` + a C compiler to build, already wired into `nativeBuildInputs`.
