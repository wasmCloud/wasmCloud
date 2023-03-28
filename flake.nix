{
  nixConfig.extra-substituters = [
    "https://wasmcloud.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-public-keys = [
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
  ];

  inputs.fenix.url = github:nix-community/fenix/monthly;
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
        } @ args:
          with pkgsCross;
          with pkgs.lib; {
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
