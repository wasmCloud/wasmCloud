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

  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = github:rvolosatovs/nixify;
  inputs.nixlib.url = github:nix-community/nixpkgs.lib;
  inputs.wasmcloud-component-adapters.inputs.nixify.follows = "nixify";
  inputs.wasmcloud-component-adapters.url = github:wasmCloud/wasmcloud-component-adapters/v0.3.0;
  inputs.wit-deps.inputs.nixify.follows = "nixify";
  inputs.wit-deps.inputs.nixlib.follows = "nixlib";
  inputs.wit-deps.url = github:bytecodealliance/wit-deps/v0.3.3;

  outputs = {
    nixify,
    nixlib,
    wasmcloud-component-adapters,
    wit-deps,
    ...
  }:
    with builtins;
    with nixlib.lib;
    with nixify.lib;
      rust.mkFlake {
        src = ./.;

        overlays = [
          wit-deps.overlays.default
        ];

        excludePaths = let
          washboardExclude = map (name: "washboard-ui/${name}") (remove "dist" (attrNames (readDir ./washboard-ui)));
        in
          [
            ".devcontainer"
            ".envrc"
            ".github"
            ".gitignore"
            "ADOPTERS.md"
            "adr"
            "awesome-wasmcloud"
            "chart"
            "CODE_OF_CONDUCT.md"
            "CODEOWNERS"
            "CONTRIBUTING.md"
            "CONTRIBUTION_LADDER.md"
            "crates/wash-cli/.devcontainer"
            "crates/wash-cli/build"
            "crates/wash-cli/Completions.md"
            "crates/wash-cli/CONTRIBUTING.md"
            "crates/wash-cli/Dockerfile"
            "crates/wash-cli/docs"
            "crates/wash-cli/Makefile"
            "crates/wash-cli/snap"
            "crates/wash-cli/tools"
            "Dockerfile"
            "flake.nix"
            "garnix.yaml"
            "GOVERNANCE.md"
            "LICENSE"
            "OWNERS"
            "README.md"
            "ROADMAP.md"
            "rust-toolchain.toml"
            "sample-manifest.yaml"
            "SECURITY.md"
          ]
          ++ washboardExclude;

        doCheck = false; # testing is performed in checks via `nextest`

        targets.armv7-unknown-linux-musleabihf = false;
        targets.wasm32-wasi = false;

        build.packages = [
          "wash-cli"
          "wasmcloud"
        ];

        clippy.allTargets = true;
        clippy.deny = ["warnings"];
        clippy.workspace = true;

        test.allTargets = true;
        test.excludes = [
          "wash-cli"
          "wash-lib"
        ];
        test.workspace = true;

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        }: {
          buildInputs ? [],
          depsBuildBuild ? [],
          nativeBuildInputs ? [],
          nativeCheckInputs ? [],
          preCheck ? "",
          ...
        } @ args: let
          cargoLock.root = readTOML ./Cargo.lock;

          cargoLock.actors-rust = readTOML ./tests/actors/rust/Cargo.lock;
          cargoLock.providers-rust = readTOML ./crates/providers/Cargo.lock;
          cargoLock.tcp-component-command = readTOML ./tests/actors/rust/tcp-component-command/Cargo.lock;

          lockPackages =
            cargoLock.root.package
            ++ cargoLock.actors-rust.package
            ++ cargoLock.providers-rust.package
            ++ cargoLock.tcp-component-command.package;
        in
          with pkgsCross;
          with pkgs.lib;
            {
              WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER = wasmcloud-component-adapters.packages.${pkgs.stdenv.system}.wasi-preview1-command-component-adapter;
              WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER = wasmcloud-component-adapters.packages.${pkgs.stdenv.system}.wasi-preview1-reactor-component-adapter;

              cargoLockParsed =
                cargoLock.root
                // {
                  package = lockPackages;
                };

              buildInputs =
                buildInputs
                ++ optionals (pkgs.stdenv.hostPlatform.isDarwin && stdenv.hostPlatform.isDarwin) [
                  pkgs.darwin.apple_sdk.frameworks.Security
                  pkgs.libiconv
                  pkgs.xcbuild.xcrun
                ];

              nativeBuildInputs =
                nativeBuildInputs
                ++ [
                  pkgs.protobuf # prost build dependency
                ];
            }
            // optionalAttrs (args ? cargoArtifacts) {
              depsBuildBuild =
                depsBuildBuild
                ++ optionals (pkgs.stdenv.hostPlatform.isDarwin && stdenv.hostPlatform.isDarwin) [
                  darwin.apple_sdk.frameworks.CoreFoundation
                  libiconv
                ];

              nativeCheckInputs =
                nativeCheckInputs
                ++ [
                  pkgs.nats-server
                  pkgs.redis
                ];

              preCheck =
                preCheck
                # See https://github.com/nextest-rs/nextest/issues/267
                + optionalString (pkgs.stdenv.hostPlatform.isDarwin && stdenv.hostPlatform.isDarwin) ''
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

          pullDebian = {
            imageDigest,
            sha256,
          }:
            pkgs.dockerTools.pullImage {
              inherit
                imageDigest
                sha256
                ;

              imageName = "debian";
              finalImageTag = "12.2-slim";
              finalImageName = "debian";
            };

          debian.aarch64 = pullDebian {
            imageDigest = "sha256:9ccb91746bf0b2e3e82b2dd37069ef9b358cb7d813217ea3fa430b940fc5dac3";
            sha256 = "sha256-cb2lPuBXaQGMrVmvp/Gq0/PtNuTtlZzUmF3S+4jHVtQ=";
          };
          debian.x86_64 = pullDebian {
            imageDigest = "sha256:ea5ad531efe1ac11ff69395d032909baf423b8b88e9aade07e11b40b2e5a1338";
            sha256 = "sha256-k+x4aUW10YAQ7X20xxJxqW57y2k20sc4e7unh/kqQZQ=";
          };

          buildImage = {
            fromImage ? null,
            bin,
            architecture,
          }: let
            copyToRoot = pkgs.buildEnv {
              name = "wasmcloud";
              extraPrefix = "/usr"; # /bin is a symlink to /usr/bin on Debian, add a prefix to avoid replacing original `/bin`
              paths = [
                bin

                pkgs.dockerTools.caCertificates
              ];
              postBuild = ''
                mv $out/usr/etc $out/etc
              '';
            };
          in
            pkgs.dockerTools.buildImage {
              inherit
                architecture
                fromImage
                copyToRoot
                ;

              name = "wasmcloud";
              tag = "${bin.version}-${bin.passthru.target}";
              config.Cmd = ["wasmcloud"];
              config.Env = ["PATH=${bin}/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"];
            };

          wasmcloud-aarch64-unknown-linux-musl-oci-debian = buildImage {
            bin = packages.wasmcloud-aarch64-unknown-linux-musl;
            fromImage = debian.aarch64;
            architecture = "arm64";
          };
          wasmcloud-x86_64-unknown-linux-musl-oci-debian = buildImage {
            bin = packages.wasmcloud-x86_64-unknown-linux-musl;
            fromImage = debian.x86_64;
            architecture = "amd64";
          };

          build-wasmcloud-oci-debian = pkgs.writeShellScriptBin "build-wasmcloud-oci-debian" ''
            set -xe

            build() {
              ${pkgs.buildah}/bin/buildah manifest create "''${1}"

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wasmcloud-aarch64-unknown-linux-musl-oci-debian}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wasmcloud-aarch64-unknown-linux-musl-oci-debian}

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wasmcloud-x86_64-unknown-linux-musl-oci-debian}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wasmcloud-x86_64-unknown-linux-musl-oci-debian}
            }
            build "''${1:-wasmcloud:debian}"
          '';
        in
          packages
          // {
            inherit
              build-wasmcloud-oci-debian
              wasmcloud-aarch64-unknown-linux-musl-oci-debian
              wasmcloud-x86_64-unknown-linux-musl-oci-debian
              ;

            rust = hostRustToolchain;
          };

        withDevShells = {
          devShells,
          pkgs,
          ...
        }:
          extendDerivations {
            buildInputs = [
              pkgs.buildah
              pkgs.cargo-audit
              pkgs.nats-server
              pkgs.protobuf # prost build dependency
              pkgs.redis
              pkgs.tinygo
              pkgs.wit-deps
            ];
          }
          devShells;
      };
}
