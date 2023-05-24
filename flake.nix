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

        doCheck = false; # testing is performed in checks via `nextest`

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        }: {
          buildInputs ? [],
          nativeBuildInputs ? [],
          depsBuildBuild ? [],
          ...
        } @ args:
          with pkgsCross;
          with pkgs.lib; {
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
          "tests/integration_get.rs"
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
