{
  description = "Best Logs: a Twitch chat-log aggregator (Rust + axum backend, SvelteKit frontend embedded in the binary)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        inherit (pkgs) lib;

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        version = cargoToml.package.version;

        # Built once and reused for both `nix build` (embedded into the Rust
        # binary at compile time via rust-embed) and `nix develop` (where you
        # rebuild it yourself with `npm run build`).
        frontend = pkgs.buildNpmPackage {
          pname = "bestlogs-rs-frontend";
          inherit version;
          src = ./frontend;
          npmDepsHash = "sha256-DiYBTgT6SJxn4+2aLd7D97iMFZik7ulY1R7I4y06OWo=";

          installPhase = ''
            runHook preInstall
            cp -r build $out
            runHook postInstall
          '';
        };

        # Only what the Rust build actually needs, so editing frontend/ or
        # scratch files elsewhere doesn't bust the backend's build cache.
        backendSrc = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./build.rs
            ./example_config.json
            ./src
          ];
        };
      in
      {
        packages = {
          inherit frontend;

          default = pkgs.rustPlatform.buildRustPackage {
            pname = "bestlogs-rs";
            inherit version;
            src = backendSrc;

            cargoLock.lockFile = ./Cargo.lock;

            # aws-lc-rs (reqwest's rustls crypto provider) needs cmake + a C
            # compiler to build its vendored C code.
            nativeBuildInputs = [
              pkgs.cmake
              pkgs.pkg-config
            ];

            # Security/SystemConfiguration are provided by the platform SDK
            # automatically; only libiconv needs adding explicitly.
            buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];

            # backendSrc deliberately has no .git in it, so build.rs can't ask
            # git for the revision and would otherwise stamp the binary
            # "unknown" — /meta and the site footer show this. shortRev is set
            # only for a clean tree; a dirty checkout gets dirtyShortRev (with
            # a "-dirty" suffix), and a non-git source neither.
            GIT_COMMIT_HASH = self.shortRev or self.dirtyShortRev or "unknown";

            # rust-embed reads this directory at compile time.
            preBuild = ''
              mkdir -p frontend
              cp -r ${frontend} frontend/build
            '';

            meta = {
              description = "Twitch chat-log aggregator, ported from ZonianMidian/best-logs";
              homepage = "https://github.com/Julia-Roman/bestlogs-rs";
              license = lib.licenses.mit;
              mainProgram = "bestlogs-rs";
            };
          };
        };

        apps.default = flake-utils.lib.mkApp { drv = self.packages.${system}.default; };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.cmake
            pkgs.pkg-config
            pkgs.nodejs_22
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };

        formatter = pkgs.nixfmt;
      }
    )
    // {
      # Not per-system: importing this into a NixOS config pulls in the
      # `system.pkgs.system`-appropriate package automatically.
      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.bestlogs-rs;
          configDir = pkgs.writeTextDir "config.json" (builtins.toJSON cfg.settings);
        in
        {
          options.services.bestlogs-rs = {
            enable = lib.mkEnableOption "the Best Logs Twitch chat-log aggregator";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "bestlogs-rs package to run.";
            };

            settings = lib.mkOption {
              type = (pkgs.formats.json { }).type;
              default = { };
              example = {
                port = 2028;
                instance.maintainer = "yourname";
              };
              description = ''
                Contents of config.json (see example_config.json for the full
                schema). Merged over the built-in defaults at startup.
              '';
            };

            environmentFile = lib.mkOption {
              type = lib.types.nullOr lib.types.path;
              default = null;
              example = "/run/secrets/bestlogs-rs.env";
              description = ''
                Path to a systemd `EnvironmentFile` (`KEY=value` lines) holding
                secrets that must not end up in `settings`, since that is
                serialised into the world-readable Nix store. Currently only
                `BESTLOGS_UMAMI_TOKEN`, which overrides `umamiStats.token`.

                Read by systemd itself before privileges are dropped, so the
                file can stay root-owned despite `DynamicUser`.
              '';
            };

            openFirewall = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Open the configured port in the firewall.";
            };

            restartIfChanged = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Automatically restart the service if changed.";
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.services.bestlogs-rs = {
              description = "Best Logs";
              wantedBy = [ "multi-user.target" ];
              after = [ "network-online.target" ];
              wants = [ "network-online.target" ];
              restartIfChanged = cfg.restartIfChanged;
              serviceConfig = {
                ExecStart = lib.getExe cfg.package;
                WorkingDirectory = configDir;
                EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;
                DynamicUser = true;
                Restart = "on-failure";
                RestartSec = 5;
                # Twitch/justlog/rustlog/recent-messages instances are all
                # queried over HTTPS, so the service needs outbound network.
                PrivateNetwork = false;
                NoNewPrivileges = true;
                ProtectSystem = "strict";
                ProtectHome = true;
              };
            };

            networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [
              (cfg.settings.port or 2028)
            ];
          };
        };
    };
}
