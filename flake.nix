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
  inputs.wit-deps.inputs.nixify.follows = "nixify";
  inputs.wit-deps.inputs.nixlib.follows = "nixlib";
  inputs.wit-deps.url = "github:bytecodealliance/wit-deps/v0.4.0";

  outputs = {
    nixify,
    nixlib,
    wit-deps,
    ...
  }:
    with builtins;
    with nixlib.lib;
    with nixify.lib;
      rust.mkFlake {
        src = ./.;

        nixpkgsConfig.allowUnfree = true;

        overlays = [
          wit-deps.overlays.default
        ];

        excludePaths = let
          washboardExclude = map (name: "typescript/${name}") (remove "dist" (attrNames (readDir ./typescript)));
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
          "secrets-nats-kv"
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

            cargoLock.actors-rust = readTOML ./tests/components/rust/Cargo.lock;

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
                  pkgs.minio
                  pkgs.vault
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

          pullWolfi = {
            imageDigest,
            sha256,
          }:
            pkgs.dockerTools.pullImage {
              inherit
                imageDigest
                sha256
                ;

              imageName = "cgr.dev/chainguard/wolfi-base";
              finalImageTag = "latest";
              finalImageName = "cgr.dev/chainguard/wolfi-base";
            };

          wolfi.aarch64 = pullWolfi {
            imageDigest = "sha256:e8c9aae5f4f1cddf9ed0449f626772a55a1a45e86155698c9d6e34c862b79736";
            sha256 = "0wjpvlnashghp5d9vxj9s6685rvahxvd7ygndsjmq3nr9h1xbw2x";
          };
          wolfi.x86_64 = pullWolfi {
            imageDigest = "sha256:4e00b653d9ad1cc7c82856d827ec003e496747d608fd842196fc888f39ddc59d";
            sha256 = "1rbm5afznmbhrx8xhkx7pbxkjmw1aqxfh3wwm7sdf9w9n1qbjbw7";
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
          wash-aarch64-unknown-linux-musl-oci-wolfi = buildWashImage {
            pkg = packages.wasmcloud-aarch64-unknown-linux-musl;
            fromImage = wolfi.aarch64;
            architecture = "arm64";
          };
          wash-x86_64-unknown-linux-musl-oci-wolfi = buildWashImage {
            pkg = packages.wasmcloud-x86_64-unknown-linux-musl;
            fromImage = wolfi.x86_64;
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
          wasmcloud-aarch64-unknown-linux-musl-oci-wolfi = buildWasmcloudImage {
            pkg = packages.wasmcloud-aarch64-unknown-linux-musl;
            fromImage = wolfi.aarch64;
            architecture = "arm64";
          };
          wasmcloud-x86_64-unknown-linux-musl-oci-wolfi = buildWasmcloudImage {
            pkg = packages.wasmcloud-x86_64-unknown-linux-musl;
            fromImage = wolfi.x86_64;
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
          build-wash-oci-wolfi = pkgs.writeShellScriptBin "build-wash-oci-wolfi" ''
            set -xe

            build() {
              ${pkgs.buildah}/bin/buildah manifest create "''${1}"

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wash-aarch64-unknown-linux-musl-oci-wolfi}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wash-aarch64-unknown-linux-musl-oci-wolfi}

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wash-x86_64-unknown-linux-musl-oci-wolfi}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wash-x86_64-unknown-linux-musl-oci-wolfi}
            }
            build "''${1:-wash:wolfi}"
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
          build-wasmcloud-oci-wolfi = pkgs.writeShellScriptBin "build-wasmcloud-oci-wolfi" ''
            set -xe

            build() {
              ${pkgs.buildah}/bin/buildah manifest create "''${1}"

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wasmcloud-aarch64-unknown-linux-musl-oci-wolfi}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wasmcloud-aarch64-unknown-linux-musl-oci-wolfi}

              ${pkgs.buildah}/bin/buildah manifest add "''${1}" docker-archive:${wasmcloud-x86_64-unknown-linux-musl-oci-wolfi}
              ${pkgs.buildah}/bin/buildah pull docker-archive:${wasmcloud-x86_64-unknown-linux-musl-oci-wolfi}
            }
            build "''${1:-wasmcloud:wolfi}"
          '';
        in
          packages
          // {
            inherit
              build-wash-oci-debian
              build-wash-oci-wolfi
              build-wasmcloud-oci-debian
              build-wasmcloud-oci-wolfi
              wash-aarch64-unknown-linux-musl-oci-debian
              wash-aarch64-unknown-linux-musl-oci-wolfi
              wash-x86_64-unknown-linux-musl-oci-debian
              wash-x86_64-unknown-linux-musl-oci-wolfi
              wasmcloud-aarch64-unknown-linux-gnu-fhs
              wasmcloud-aarch64-unknown-linux-musl-oci-debian
              wasmcloud-aarch64-unknown-linux-musl-oci-wolfi
              wasmcloud-riscv64gc-unknown-linux-gnu-fhs
              wasmcloud-x86_64-unknown-linux-gnu-fhs
              wasmcloud-x86_64-unknown-linux-musl-oci-debian
              wasmcloud-x86_64-unknown-linux-musl-oci-wolfi
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
