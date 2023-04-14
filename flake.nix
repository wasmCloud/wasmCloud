{
  nixConfig.extra-substituters = [
    "https://wasmcloud.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-public-keys = [
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
  ];

  # TODO: Remove once https://github.com/bytecodealliance/preview2-prototyping/pull/136 is merged
  inputs.crane.url = github:rvolosatovs/crane/feat/wit;
  inputs.fenix.url = github:nix-community/fenix/monthly;
  inputs.nixify.inputs.crane.follows = "crane";
  inputs.nixify.inputs.fenix.follows = "fenix";
  inputs.nixify.url = github:rvolosatovs/nixify;

  outputs = {nixify, ...}:
    with nixify.lib;
      rust.mkFlake {
        src = ./.;

        excludePaths = [
          ".archived"
          ".gitignore"
          "awesome-wasmcloud"
          "CODE_OF_CONDUCT.md"
          "CONTRIBUTING.md"
          "CONTRIBUTION_LADDER.md"
          "flake.lock"
          "flake.nix"
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
        targets.x86_64-pc-windows-gnu = false;

        test.allTargets = true;
        test.workspace = true;

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        }: {
          buildInputs ? [],
          depsBuildBuild ? [],
          ...
        } @ args: let
          cargoLock.root = readTOML ./Cargo.lock;
          cargoLock.actors-rust = readTOML ./tests/actors/rust/Cargo.lock;
          cargoLock.wasi-adapter = readTOML ./tests/wasi-adapter/Cargo.lock;

          lockPackages = cargoLock.root.package ++ cargoLock.actors-rust.package ++ cargoLock.wasi-adapter.package;
        in
          with pkgsCross;
          with pkgs.lib; {
            cargoLockParsed =
              cargoLock.root
              // {
                package = lockPackages;
              };

            buildInputs =
              buildInputs
              ++ optional stdenv.targetPlatform.isDarwin pkgs.libiconv;

            depsBuildBuild =
              depsBuildBuild
              ++ optionals stdenv.targetPlatform.isDarwin [
                darwin.apple_sdk.frameworks.CoreFoundation
                libiconv
              ];
          };
      };
}
