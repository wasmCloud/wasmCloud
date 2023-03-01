{
  description = "wash - wasmCloud Shell";

  inputs.nixify.url = github:rvolosatovs/nixify;

  outputs = {
    self,
    nixify,
  }:
    with nixify.lib;
      rust.mkFlake {
        name = "wash";
        src = ./.;

        targets.wasm32-wasi = false; # `wash` does not compile for WASI

        buildOverrides = {
          pkgs,
          buildInputs ? [],
          nativeBuildInputs ? [],
          depsBuildBuild ? [],
          ...
        } @ args:
          with pkgs.lib;
          with (args.pkgsCross or pkgs); {
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
              pkgs.git # required for integration tests
              pkgs.tinygo # required for integration tests
              pkgs.protobuf # build dependency of prost-build v0.9.0
            ];
          }
          devShells;

        excludePaths = [
          ".devcontainer"
          ".github"
          ".gitignore"
          ".pre-commit-config.yaml"
          "Completions.md"
          "Dockerfile"
          "flake.lock"
          "flake.nix"
          "LICENSE"
          "Makefile"
          "README.md"
          "rust-toolchain.toml"
          "sample-manifest.yaml"
          "snap"
          "tools"

          # Exclude tests, which require either:
          # - non-deterministic networking, which is not available within Nix sandbox
          # - external services running, which would require a more involved setup
          "tests/integration_build.rs"
          "tests/integration_claims.rs"
          "tests/integration_inspect.rs"
          "tests/integration_keys.rs"
          "tests/integration_link.rs"
          "tests/integration_par.rs"
          "tests/integration_reg.rs"
          "tests/integration_start.rs"
          "tests/integration_stop.rs"
          "tests/integration_up.rs"
        ];
      };
}
