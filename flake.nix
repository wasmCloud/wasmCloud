{
  nixConfig.extra-substituters = [
    "https://wasmcloud.cachix.org"
    "https://nixify.cachix.org"
    "https://crane.cachix.org"
    "https://bytecodealliance.cachix.org"
    "https://nix-community.cachix.org"
    "https://cache.garnix.io"
  ];
  nixConfig.extra-trusted-public-keys = [
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nixify.cachix.org-1:95SiUQuf8Ij0hwDweALJsLtnMyv/otZamWNRp1Q1pXw="
    "crane.cachix.org-1:8Scfpmn9w+hGdXH/Q9tTLiYAE/2dnJYRJP7kl80GuRk="
    "bytecodealliance.cachix.org-1:0SBgh//n2n0heh0sDFhTm+ZKBRy2sInakzFGfzN531Y="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
  ];

  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = "github:rvolosatovs/nixify";
  inputs.nixlib.url = "github:nix-community/nixpkgs.lib";
  inputs.wasmcloud-component-adapters.inputs.nixify.follows = "nixify";
  inputs.wasmcloud-component-adapters.url = "github:wasmCloud/wasmcloud-component-adapters/v0.9.0";
  inputs.wit-deps.inputs.nixify.follows = "nixify";
  inputs.wit-deps.inputs.nixlib.follows = "nixlib";
  inputs.wit-deps.url = "github:bytecodealliance/wit-deps/v0.3.5";

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

        targets.arm-unknown-linux-gnueabihf = false;
        targets.arm-unknown-linux-musleabihf = false;
        targets.armv7-unknown-linux-gnueabihf = false;
        targets.armv7-unknown-linux-musleabihf = false;
        targets.powerpc64le-unknown-linux-gnu = false;
        targets.s390x-unknown-linux-gnu = false;
        targets.wasm32-unknown-unknown = false;
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
          "wasmcloud-provider-blobstore-s3" # TODO: Make the test self-contained and reenable
          "wasmcloud-provider-messaging-nats" # tests appear to be broken
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
          ...
        } @ args:
          with pkgs.lib; let
            cargoLock.root = readTOML ./Cargo.lock;

            cargoLock.actors-rust = readTOML ./tests/actors/rust/Cargo.lock;

            lockPackages =
              cargoLock.root.package
              ++ cargoLock.actors-rust.package;

            # deduplicate lockPackages by $name:$version:$checksum
            lockPackages' = listToAttrs (
              map (
                {
                  name,
                  version,
                  checksum ? "no-hash",
                  ...
                } @ pkg:
                  nameValuePair "${name}:${version}:${checksum}" pkg
              )
              lockPackages
            );

            cargoLockParsed =
              cargoLock.root
              // {
                package = attrValues lockPackages';
              };

            darwin2darwin = pkgs.stdenv.hostPlatform.isDarwin && pkgsCross.stdenv.hostPlatform.isDarwin;

            depsBuildBuild' =
              depsBuildBuild
              ++ optional pkgs.stdenv.hostPlatform.isDarwin pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
              ++ optional darwin2darwin pkgs.xcbuild.xcrun;
          in
            {
              inherit
                cargoLockParsed
                ;
              WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER = wasmcloud-component-adapters.packages.${pkgs.stdenv.system}.wasi-preview1-command-component-adapter;
              WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER = wasmcloud-component-adapters.packages.${pkgs.stdenv.system}.wasi-preview1-reactor-component-adapter;

              cargoExtraArgs = ""; # disable `--locked` passed by default by crane

              buildInputs =
                buildInputs
                ++ optional pkgs.stdenv.hostPlatform.isDarwin pkgs.libiconv;

              depsBuildBuild = depsBuildBuild';
            }
            // optionalAttrs (args ? cargoArtifacts) {
              depsBuildBuild =
                depsBuildBuild'
                ++ optionals darwin2darwin [
                  pkgs.darwin.apple_sdk.frameworks.CoreFoundation
                  pkgs.darwin.apple_sdk.frameworks.CoreServices
                ];

              nativeCheckInputs =
                nativeCheckInputs
                ++ [
                  pkgs.nats-server
                  pkgs.redis
                  pkgs.vault
                  pkgs.minio
                ];
            };

        withPackages = {
          hostRustToolchain,
          packages,
          pkgs,
          ...
        }: let
          interpreters.aarch64-unknown-linux-gnu = "/lib/ld-linux-aarch64.so.1";
          interpreters.riscv64gc-unknown-linux-gnu = "/lib/ld-linux-riscv64-lp64d.so.1";
          interpreters.x86_64-unknown-linux-gnu = "/lib64/ld-linux-x86-64.so.2";

          mkFHS = {
            name,
            src,
            interpreter,
          }:
            pkgs.stdenv.mkDerivation {
              inherit
                name
                src
                ;

              buildInputs = [
                pkgs.patchelf
              ];

              dontBuild = true;
              dontFixup = true;

              installPhase = ''
                runHook preInstall

                for p in $(find . -type f); do
                  # https://en.wikipedia.org/wiki/Executable_and_Linkable_Format#File_header
                  if head -c 4 $p | grep $'\x7FELF' > /dev/null; then
                    patchelf --set-rpath /lib $p || true
                    patchelf --set-interpreter ${interpreter} $p || true
                  fi
                done

                mkdir -p $out
                cp -R * $out

                runHook postInstall
              '';
            };

          wasmcloud-aarch64-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmcloud-aarch64-unknown-linux-gnu-fhs";
            src = packages.wasmcloud-aarch64-unknown-linux-gnu;
            interpreter = interpreters.aarch64-unknown-linux-gnu;
          };

          wasmcloud-riscv64gc-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmcloud-riscv64gc-unknown-linux-gnu-fhs";
            src = packages.wasmcloud-riscv64gc-unknown-linux-gnu;
            interpreter = interpreters.riscv64gc-unknown-linux-gnu;
          };

          wasmcloud-x86_64-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmcloud-x86_64-unknown-linux-gnu-fhs";
            src = packages.wasmcloud-x86_64-unknown-linux-gnu;
            interpreter = interpreters.x86_64-unknown-linux-gnu;
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
            pkg,
            name,
            architecture,
            description,
          }: let
            # ensure that only the binary corresponding to `$name` is copied to the image
            bin = pkgs.runCommandLocal name {} ''
              mkdir -p $out/bin
              cp ${pkg}/bin/${name} $out/bin/${name}
            '';

            copyToRoot = pkgs.buildEnv {
              inherit name;
              extraPrefix = "/usr"; # /bin is a symlink to /usr/bin on Debian, add a prefix to avoid replacing original `/bin`
              paths = [
                bin

                pkgs.dockerTools.caCertificates
              ];
              postBuild = ''
                mv $out/usr/etc $out/etc
              '';
            };

            version =
              if name == "wasmcloud"
              then (readTOML ./Cargo.toml).package.version
              else if name == "wash"
              then (readTOML ./crates/wash-cli/Cargo.toml).package.version
              else throw "unsupported binary `${name}`";
          in
            pkgs.dockerTools.buildImage {
              inherit
                architecture
                copyToRoot
                fromImage
                name
                ;
              tag = architecture;

              config.Cmd = [name];
              config.Labels."org.opencontainers.image.description" = description;
              config.Labels."org.opencontainers.image.source" = "https://github.com/wasmCloud/wasmCloud";
              config.Labels."org.opencontainers.image.title" = name;
              config.Labels."org.opencontainers.image.vendor" = "wasmCloud";
              config.Labels."org.opencontainers.image.version" = version;
            };

          buildWashImage = {
            pkg,
            fromImage,
            architecture,
          }:
            buildImage {
              inherit
                architecture
                fromImage
                pkg
                ;
              name = "wash";
              description = "WAsmcloud SHell";
            };
          wash-aarch64-unknown-linux-musl-oci-debian = buildWashImage {
            pkg = packages.wasmcloud-aarch64-unknown-linux-musl;
            fromImage = debian.aarch64;
            architecture = "arm64";
          };
          wash-x86_64-unknown-linux-musl-oci-debian = buildWashImage {
            pkg = packages.wasmcloud-x86_64-unknown-linux-musl;
            fromImage = debian.x86_64;
            architecture = "amd64";
          };

          buildWasmcloudImage = {
            pkg,
            fromImage,
            architecture,
          }:
            buildImage {
              inherit
                architecture
                fromImage
                pkg
                ;
              name = "wasmcloud";
              description = "wasmCloud host";
            };
          wasmcloud-aarch64-unknown-linux-musl-oci-debian = buildWasmcloudImage {
            pkg = packages.wasmcloud-aarch64-unknown-linux-musl;
            fromImage = debian.aarch64;
            architecture = "arm64";
          };
          wasmcloud-x86_64-unknown-linux-musl-oci-debian = buildWasmcloudImage {
            pkg = packages.wasmcloud-x86_64-unknown-linux-musl;
            fromImage = debian.x86_64;
            architecture = "amd64";
          };

          build-wash-oci-debian = pkgs.writeShellScriptBin "build-wash-oci-debian" ''
            set -xe

            build() {
              ${pkgs.buildah}/bin/buildah manifest create "''${1}"

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wash-aarch64-unknown-linux-musl-oci-debian}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wash-aarch64-unknown-linux-musl-oci-debian}

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wash-x86_64-unknown-linux-musl-oci-debian}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wash-x86_64-unknown-linux-musl-oci-debian}
            }
            build "''${1:-wash:debian}"
          '';
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
              build-wash-oci-debian
              build-wasmcloud-oci-debian
              wash-aarch64-unknown-linux-musl-oci-debian
              wash-x86_64-unknown-linux-musl-oci-debian
              wasmcloud-aarch64-unknown-linux-gnu-fhs
              wasmcloud-aarch64-unknown-linux-musl-oci-debian
              wasmcloud-riscv64gc-unknown-linux-gnu-fhs
              wasmcloud-x86_64-unknown-linux-gnu-fhs
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
              pkgs.minio
              pkgs.nats-server
              pkgs.redis
              pkgs.tinygo
              pkgs.vault
              pkgs.wit-deps
            ];
          }
          devShells;
      };
}
