{
  nixConfig.extra-substituters = [
    "https://wasmcloud.cachix.org"
    "https://nix-community.cachix.org"
    "https://cache.garnix.io"
  ];
  nixConfig.extra-trusted-public-keys = [
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
  ];

  inputs.fenix.url = github:nix-community/fenix/monthly;
  inputs.nixify.inputs.fenix.follows = "fenix";
  inputs.nixify.url = github:rvolosatovs/nixify;

  outputs = {nixify, ...}:
    with nixify.lib;
      rust.mkFlake {
        src = ./.;

        excludePaths = [
          ".github"
          ".gitignore"
          "ADOPTERS.md"
          "awesome-wasmcloud"
          "CODE_OF_CONDUCT.md"
          "CODEOWNERS"
          "CONTRIBUTING.md"
          "CONTRIBUTION_LADDER.md"
          "flake.lock"
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
          preCheck ? "",
          ...
        } @ args: let
          cargoLock.root = readTOML ./Cargo.lock;
          cargoLock.actors-rust = readTOML ./tests/actors/rust/Cargo.lock;
          cargoLock.wasi-adapter = readTOML ./tests/wasi-adapter/Cargo.lock;

          lockPackages = cargoLock.root.package ++ cargoLock.actors-rust.package ++ cargoLock.wasi-adapter.package;
        in
          with pkgsCross;
          with pkgs.lib;
            {
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
            }
            // optionalAttrs (args ? cargoArtifacts && stdenv.hostPlatform.isDarwin) {
              # See https://github.com/nextest-rs/nextest/issues/267
              preCheck =
                preCheck
                + ''
                  export DYLD_FALLBACK_LIBRARY_PATH=$(rustc --print sysroot)/lib
                '';
            };
      };
}
