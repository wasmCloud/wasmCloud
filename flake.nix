{
  nixConfig.extra-substituters = [
    "https://wasmcloud.cachix.org"
    "https://bytecodealliance.cachix.org"
    "https://nix-community.cachix.org"
    "https://cache.garnix.io"
  ];
  nixConfig.extra-trusted-public-keys = [
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "bytecodealliance.cachix.org-1:0SBgh//n2n0heh0sDFhTm+ZKBRy2sInakzFGfzN531Y="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
  ];

  inputs.fenix.url = github:nix-community/fenix/monthly;
  inputs.nixify.inputs.fenix.follows = "fenix";
  inputs.nixify.url = github:rvolosatovs/nixify;
  inputs.wash.inputs.nixify.follows = "nixify"; # TODO: drop once updated upstream
  inputs.wash.url = github:wasmcloud/wash/v0.18.1;
  inputs.wasi-preview1-command-component-adapter.flake = false;
  inputs.wasi-preview1-command-component-adapter.url = https://github.com/bytecodealliance/wasmtime/releases/download/v10.0.1/wasi_snapshot_preview1.command.wasm;
  inputs.wasi-preview1-reactor-component-adapter.flake = false;
  inputs.wasi-preview1-reactor-component-adapter.url = https://github.com/bytecodealliance/wasmtime/releases/download/v10.0.1/wasi_snapshot_preview1.reactor.wasm;
  inputs.wit-deps.inputs.nixify.follows = "nixify"; # TODO: drop once updated upstream
  inputs.wit-deps.url = github:bytecodealliance/wit-deps/v0.3.2;

  outputs = {
    nixify,
    wash,
    wasi-preview1-command-component-adapter,
    wasi-preview1-reactor-component-adapter,
    wit-deps,
    ...
  }:
    with nixify.lib; let
      WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER = wasi-preview1-command-component-adapter;
      WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER = wasi-preview1-reactor-component-adapter;
    in
      rust.mkFlake {
        src = ./.;

        overlays = [
          wash.overlays.default
          wit-deps.overlays.default
        ];

        excludePaths = [
          ".github"
          ".gitignore"
          "ADOPTERS.md"
          "awesome-wasmcloud"
          "CODE_OF_CONDUCT.md"
          "CODEOWNERS"
          "CONTRIBUTING.md"
          "CONTRIBUTION_LADDER.md"
          "flake.nix"
          "garnix.yaml"
          "GOVERNANCE.md"
          "LICENSE"
          "OWNERS"
          "README.md"
          "ROADMAP.md"
          "rust-toolchain.toml"
          "SECURITY.md"
        ];

        doCheck = false; # testing is performed in checks via `nextest`

        clippy.allTargets = true;
        clippy.deny = ["warnings"];
        clippy.workspace = true;

        targets.armv7-unknown-linux-musleabihf = false;
        targets.wasm32-wasi = false;

        test.allTargets = true;
        test.workspace = true;

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        }: {
          buildInputs ? [],
          depsBuildBuild ? [],
          nativeBuildInputs ? [],
          preCheck ? "",
          ...
        } @ args: let
          cargoLock.root = readTOML ./Cargo.lock;

          cargoLock.actors-rust = readTOML ./tests/actors/rust/Cargo.lock;
          cargoLock.tcp-component-command = readTOML ./tests/actors/rust/tcp-component-command/Cargo.lock;

          lockPackages =
            cargoLock.root.package
            ++ cargoLock.actors-rust.package
            ++ cargoLock.tcp-component-command.package;
        in
          with pkgsCross;
          with pkgs.lib;
            {
              inherit
                WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER
                WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER
                ;

              cargoLockParsed =
                cargoLock.root
                // {
                  package = lockPackages;
                };

              buildInputs =
                buildInputs
                ++ optionals stdenv.hostPlatform.isDarwin [
                  pkgs.darwin.apple_sdk.frameworks.Security
                  pkgs.libiconv
                ];

              depsBuildBuild =
                depsBuildBuild
                ++ optionals stdenv.hostPlatform.isDarwin [
                  darwin.apple_sdk.frameworks.CoreFoundation
                  libiconv
                ];

              nativeBuildInputs =
                nativeBuildInputs
                ++ [
                  pkgs.protobuf # prost build dependency
                ];
            }
            // optionalAttrs (args ? cargoArtifacts && stdenv.hostPlatform.isDarwin) {
              # See https://github.com/nextest-rs/nextest/issues/267
              preCheck =
                preCheck
                + ''
                  export DYLD_FALLBACK_LIBRARY_PATH=$(rustc --print sysroot)/lib
                '';
            };

        withPackages = {
          hostRustToolchain,
          packages,
          pkgs,
          ...
        }: let
          mkAdapter = name: src:
            pkgs.stdenv.mkDerivation {
              inherit
                name
                src
                ;

              dontUnpack = true;
              dontBuild = true;

              installPhase = ''
                install $src $out
              '';
            };
        in
          packages
          // {
            rust = hostRustToolchain;

            wasi-preview1-command-component-adapter = mkAdapter "wasi-preview1-command-component-adapter" wasi-preview1-command-component-adapter;
            wasi-preview1-reactor-component-adapter = mkAdapter "wasi-preview1-reactor-component-adapter" wasi-preview1-reactor-component-adapter;
          };

        withDevShells = {
          devShells,
          pkgs,
          ...
        }:
          extendDerivations {
            buildInputs = [
              pkgs.cargo-audit
              pkgs.nats-server
              pkgs.protobuf # prost build dependency
              pkgs.wash
              pkgs.wit-deps
            ];
          }
          devShells;
      };
}
