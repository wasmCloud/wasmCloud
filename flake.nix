{
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
          "GOVERNANCE.md"
          "LICENSE"
          "OWNERS"
          "README.md"
          "ROADMAP.md"
          "rust-toolchain.toml"
          "SECURITY.md"
        ];

        buildOverrides = {
          pkgs,
          buildInputs ? [],
          nativeBuildInputs ? [],
          ...
        } @ args:
          with pkgs.lib;
          with (args.pkgsCross or pkgs); {
            buildInputs =
              buildInputs
              ++ optional stdenv.targetPlatform.isDarwin darwin.apple_sdk.frameworks.Security;

            nativeBuildInputs =
              nativeBuildInputs
              ++ [
                pkgs.protobuf # build dependency of prost-build v0.9.0
              ];
          };

        withDevShells = {
          pkgs,
          devShells,
          ...
        }:
          extendDerivations {
            buildInputs = [
              pkgs.protobuf # build dependency of prost-build v0.9.0
            ];
          }
          devShells;

        targets.wasm32-wasi = false;

        test.allTargets = true;
        test.workspace = true;

        clippy.allTargets = true;
        clippy.workspace = true;

        clippy.allow = [];
        clippy.deny = ["warnings"];
        clippy.forbid = [];
        clippy.warn = [];
      };
}
