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
  inputs.nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.wit-deps.inputs.nixify.follows = "nixify";
  inputs.wit-deps.inputs.nixlib.follows = "nixlib";
  inputs.wit-deps.url = "github:bytecodealliance/wit-deps/v0.5.0";

  outputs = {
    nixify,
    nixlib,
    nixpkgs-unstable,
    wit-deps,
    ...
  }:
    with builtins;
    with nixlib.lib;
    with nixify.lib; let
      targets.arm-unknown-linux-gnueabihf = false;
      targets.arm-unknown-linux-musleabihf = false;
      targets.armv7-unknown-linux-gnueabihf = false;
      targets.armv7-unknown-linux-musleabihf = false;
      targets.powerpc64le-unknown-linux-gnu = false;
      targets.s390x-unknown-linux-gnu = false;
      targets.wasm32-unknown-unknown = false;
      targets.wasm32-wasip1 = false;
      targets.wasm32-wasip2 = false;
    in
      rust.mkFlake {
        inherit
          targets
          ;
        src = ./.;
        name = "workspace";

        nixpkgsConfig.allowUnfree = true;

        overlays = [
          wit-deps.overlays.default
          (
            final: prev: {
              pkgsUnstable = import nixpkgs-unstable {
                inherit
                  (final.stdenv.hostPlatform)
                  system
                  ;

                inherit
                  (final)
                  config
                  ;
              };
            }
          )
        ];

        excludePaths = [
          ".devcontainer"
          ".dockerignore"
          ".envrc"
          ".github"
          ".gitignore"
          "ADOPTERS.md"
          "adr"
          "awesome-wasmcloud"
          "brand"
          "CHANGELOG.md"
          "chart"
          "charts"
          "CODE_OF_CONDUCT.md"
          "CODEOWNERS"
          "CONTRIBUTING.md"
          "CONTRIBUTION_LADDER.md"
          "crates/wash/src/cli/.devcontainer"
          "crates/wash/src/cli/build"
          "crates/wash/src/cli/Completions.md"
          "crates/wash/src/cli/CONTRIBUTING.md"
          "crates/wash/src/cli/Dockerfile"
          "crates/wash/src/cli/docs"
          "crates/wash/src/cli/Makefile"
          "crates/wash/src/cli/snap"
          "crates/wash/src/cli/tools"
          "Dockerfile"
          "flake.nix"
          "garnix.yaml"
          "GOVERNANCE.md"
          "LICENSE"
          "MAINTAINERS.md"
          "nix"
          "OWNERS"
          "performance.md"
          "README.md"
          "RELEASE.md"
          "RELEASE_RUNBOOK.md"
          "ROADMAP.md"
          "rust-toolchain.toml"
          "SECURITY.md"
        ];

        doCheck = false; # testing is performed in checks via `nextest`

        build.workspace = true;

        clippy.allTargets = true;
        clippy.deny = ["warnings"];
        clippy.workspace = true;

        test.allTargets = true;
        test.features = [
          "provider-blobstore-fs"
          "provider-blobstore-s3"
          "provider-http-client"
          "provider-http-server"
          "provider-keyvalue-redis"
          "provider-keyvalue-vault"
          "provider-messaging-kafka"
          "provider-messaging-nats"
          "wasmcloud"
        ];
        test.noDefaultFeatures = true;
        test.excludes = [
          "secrets-nats-kv"
          "wash"
          "wasmcloud-provider-blobstore-s3" # TODO: Make the test self-contained and reenable
          "wasmcloud-provider-messaging-nats" # tests appear to be broken
        ];
        test.workspace = true;

        buildOverrides = {
          craneLib,
          pkgs,
          pkgsCross ? pkgs,
          ...
        }: {nativeCheckInputs ? [], ...} @ args:
          with pkgs.lib;
            {
              cargoVendorDir = craneLib.vendorMultipleCargoDeps {
                cargoLockList = [
                  ./Cargo.lock
                  ./examples/rust/components/http-hello-world/Cargo.lock
                  ./examples/rust/components/http-keyvalue-counter/Cargo.lock
                  ./tests/components/rust/Cargo.lock
                ];
              };
            }
            // optionalAttrs (args ? cargoArtifacts) {
              nativeCheckInputs =
                nativeCheckInputs
                ++ [
                  pkgs.minio
                  pkgs.redis
                  pkgs.spire-agent
                  pkgs.spire-server
                  pkgs.vault-bin

                  pkgs.pkgsUnstable.nats-server
                ];
            };

        withPackages = {
          hostRustToolchain,
          pkgs,
          src,
          ...
        }: let
          attrs = let
            binDir = readDir ./src/bin;
            providers = concatMapAttrs (name: typ:
              optionalAttrs (hasSuffix "-provider" name && typ == "directory") {
                "${name}" =
                  rust.mkAttrs {
                    inherit
                      src
                      targets
                      ;
                    pname = name;
                    doCheck = false;
                    build.bins = [
                      name
                    ];
                    build.features = [
                      "provider-${removeSuffix "-provider" name}"
                    ];
                    build.noDefaultFeatures = true;
                    build.packages = [
                      "wasmcloud"
                    ];
                  }
                  pkgs;
              })
            binDir;
          in
            providers
            // {
              wash =
                rust.mkAttrs {
                  inherit
                    src
                    targets
                    ;
                  pname = "wash";
                  doCheck = false;
                  build.bins = [
                    "wash"
                  ];
                  build.packages = [
                    "wash"
                  ];
                }
                pkgs;
              wasmcloud =
                rust.mkAttrs {
                  inherit
                    src
                    targets
                    ;
                  pname = "wasmcloud";
                  doCheck = false;
                  build.bins = [
                    "wasmcloud"
                  ];
                  build.features = [
                    "wasmcloud"
                  ];
                  build.noDefaultFeatures = true;
                  build.packages = [
                    "wasmcloud"
                  ];
                }
                pkgs;
              secrets-nats-kv =
                rust.mkAttrs {
                  inherit
                    src
                    targets
                    ;
                  pname = "secrets-nats-kv";
                  doCheck = false;
                  build.packages = [
                    "secrets-nats-kv"
                  ];
                }
                pkgs;
            };

          interpreters.aarch64-unknown-linux-gnu = "/lib/ld-linux-aarch64.so.1";
          interpreters.riscv64gc-unknown-linux-gnu = "/lib/ld-linux-riscv64-lp64d.so.1";
          interpreters.x86_64-unknown-linux-gnu = "/lib64/ld-linux-x86-64.so.2";

          images = mapAttrs (_: pkgs.dockerTools.pullImage) (import ./nix/images);

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

          wash-aarch64-unknown-linux-gnu-fhs = mkFHS {
            name = "wash-aarch64-unknown-linux-gnu-fhs";
            src = attrs.wash.packages.wash-aarch64-unknown-linux-gnu;
            interpreter = interpreters.aarch64-unknown-linux-gnu;
          };

          wash-riscv64gc-unknown-linux-gnu-fhs = mkFHS {
            name = "wash-riscv64gc-unknown-linux-gnu-fhs";
            src = attrs.wash.packages.wash-riscv64gc-unknown-linux-gnu;
            interpreter = interpreters.riscv64gc-unknown-linux-gnu;
          };

          wash-x86_64-unknown-linux-gnu-fhs = mkFHS {
            name = "wash-x86_64-unknown-linux-gnu-fhs";
            src = attrs.wash.packages.wash-x86_64-unknown-linux-gnu;
            interpreter = interpreters.x86_64-unknown-linux-gnu;
          };

          wasmcloud-aarch64-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmcloud-aarch64-unknown-linux-gnu-fhs";
            src = attrs.wasmcloud.packages.wasmcloud-aarch64-unknown-linux-gnu;
            interpreter = interpreters.aarch64-unknown-linux-gnu;
          };

          wasmcloud-riscv64gc-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmcloud-riscv64gc-unknown-linux-gnu-fhs";
            src = attrs.wasmcloud.packages.wasmcloud-riscv64gc-unknown-linux-gnu;
            interpreter = interpreters.riscv64gc-unknown-linux-gnu;
          };

          wasmcloud-x86_64-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmcloud-x86_64-unknown-linux-gnu-fhs";
            src = attrs.wasmcloud.packages.wasmcloud-x86_64-unknown-linux-gnu;
            interpreter = interpreters.x86_64-unknown-linux-gnu;
          };

          buildImage = {
            fromImage ? null,
            pkg,
            name,
            architecture,
            description,
            user ? null,
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
              then (readTOML ./crates/wash/Cargo.toml).package.version
              else if name == "secrets-nats-kv"
              then (readTOML ./crates/secrets-nats-kv/Cargo.toml).package.version
              else throw "unsupported binary `${name}`";

            config =
              {
                Cmd = [name];
                Labels."org.opencontainers.image.description" = description;
                Labels."org.opencontainers.image.source" = "https://github.com/wasmCloud/wasmCloud";
                Labels."org.opencontainers.image.title" = name;
                Labels."org.opencontainers.image.vendor" = "wasmCloud";
                Labels."org.opencontainers.image.version" = version;
              }
              // optionalAttrs (user != null) {
                User = user;
              };
          in
            pkgs.dockerTools.buildImage {
              inherit
                architecture
                config
                copyToRoot
                fromImage
                name
                ;
              created = "now";
              tag = architecture;
            };

          imageArgs.bin.wash.description = "WAsmcloud SHell";
          imageArgs.bin.wash.name = "wash";
          imageArgs.bin.wasmcloud.description = "wasmCloud host";
          imageArgs.bin.wasmcloud.name = "wasmcloud";
          imageArgs.bin.secrets-nats-kv.description = "wasmCloud Secrets implementation using NATS";
          imageArgs.bin.secrets-nats-kv.name = "secrets-nats-kv";
          imageArgs.config.wolfi.user = "65532:65532"; # nonroot:x:65532:65532
          imageArgs.image.debian-amd64.architecture = "amd64";
          imageArgs.image.debian-amd64.fromImage = images.debian-amd64;
          imageArgs.image.debian-arm64.architecture = "arm64";
          imageArgs.image.debian-arm64.fromImage = images.debian-arm64;
          imageArgs.image.wolfi-amd64.architecture = "amd64";
          imageArgs.image.wolfi-amd64.fromImage = images.wolfi-amd64;
          imageArgs.image.wolfi-arm64.architecture = "arm64";
          imageArgs.image.wolfi-arm64.fromImage = images.wolfi-arm64;

          wash-aarch64-unknown-linux-musl-oci-debian = buildImage (
            {
              pkg = attrs.wash.packages.wash-aarch64-unknown-linux-musl;
            }
            // imageArgs.bin.wash
            // imageArgs.image.debian-arm64
          );
          wash-x86_64-unknown-linux-musl-oci-debian = buildImage (
            {
              pkg = attrs.wash.packages.wash-x86_64-unknown-linux-musl;
            }
            // imageArgs.bin.wash
            // imageArgs.image.debian-amd64
          );
          wash-aarch64-unknown-linux-musl-oci-wolfi = buildImage (
            {
              pkg = attrs.wash.packages.wash-aarch64-unknown-linux-musl;
            }
            // imageArgs.bin.wash
            // imageArgs.config.wolfi
            // imageArgs.image.wolfi-arm64
          );
          wash-x86_64-unknown-linux-musl-oci-wolfi = buildImage (
            {
              pkg = attrs.wash.packages.wash-x86_64-unknown-linux-musl;
            }
            // imageArgs.bin.wash
            // imageArgs.config.wolfi
            // imageArgs.image.wolfi-amd64
          );

          wasmcloud-aarch64-unknown-linux-musl-oci-debian = buildImage (
            {
              pkg = attrs.wasmcloud.packages.wasmcloud-aarch64-unknown-linux-musl;
            }
            // imageArgs.bin.wasmcloud
            // imageArgs.image.debian-arm64
          );
          wasmcloud-x86_64-unknown-linux-musl-oci-debian = buildImage (
            {
              pkg = attrs.wasmcloud.packages.wasmcloud-x86_64-unknown-linux-musl;
            }
            // imageArgs.bin.wasmcloud
            // imageArgs.image.debian-amd64
          );
          wasmcloud-aarch64-unknown-linux-musl-oci-wolfi = buildImage (
            {
              pkg = attrs.wasmcloud.packages.wasmcloud-aarch64-unknown-linux-musl;
            }
            // imageArgs.bin.wasmcloud
            // imageArgs.config.wolfi
            // imageArgs.image.wolfi-arm64
          );
          wasmcloud-x86_64-unknown-linux-musl-oci-wolfi = buildImage (
            {
              pkg = attrs.wasmcloud.packages.wasmcloud-x86_64-unknown-linux-musl;
            }
            // imageArgs.bin.wasmcloud
            // imageArgs.config.wolfi
            // imageArgs.image.wolfi-amd64
          );

          secrets-nats-kv-aarch64-unknown-linux-musl-oci-debian = buildImage (
            {
              pkg = attrs.secrets-nats-kv.packages.secrets-nats-kv-aarch64-unknown-linux-musl;
            }
            // imageArgs.bin.secrets-nats-kv
            // imageArgs.image.debian-arm64
          );
          secrets-nats-kv-x86_64-unknown-linux-musl-oci-debian = buildImage (
            {
              pkg = attrs.secrets-nats-kv.packages.secrets-nats-kv-x86_64-unknown-linux-musl;
            }
            // imageArgs.bin.secrets-nats-kv
            // imageArgs.image.debian-amd64
          );
          secrets-nats-kv-aarch64-unknown-linux-musl-oci-wolfi = buildImage (
            {
              pkg = attrs.secrets-nats-kv.packages.secrets-nats-kv-aarch64-unknown-linux-musl;
            }
            // imageArgs.bin.secrets-nats-kv
            // imageArgs.config.wolfi
            // imageArgs.image.wolfi-arm64
          );
          secrets-nats-kv-x86_64-unknown-linux-musl-oci-wolfi = buildImage (
            {
              pkg = attrs.secrets-nats-kv.packages.secrets-nats-kv-x86_64-unknown-linux-musl;
            }
            // imageArgs.bin.secrets-nats-kv
            // imageArgs.config.wolfi
            // imageArgs.image.wolfi-amd64
          );

          buildImageManifest = pkg:
            pkgs.runCommand "${pkg.imageName}-${pkg.imageTag}-manifest.json"
            {
              nativeBuildInputs = [pkgs.skopeo];
            }
            ''
              skopeo inspect --raw --tmpdir="$(mktemp -d)" docker-archive://${pkg} > $out
            '';

          buildImageDir = pkg:
            pkgs.runCommand "${pkg.imageName}-${pkg.imageTag}-dir"
            {
              nativeBuildInputs = [pkgs.skopeo];
            }
            ''
              skopeo copy --insecure-policy --tmpdir="$(mktemp -d)" docker-archive://${pkg} dir:$out
            '';

          buildMultiArchImage = {
            name,
            base,
            amd64,
            arm64,
          }: let
            manifests.amd64 = buildImageManifest amd64;
            manifests.arm64 = buildImageManifest arm64;

            manifest = pkgs.writeText "${name}-oci-${base}-manifest.json" (toJSON {
              schemaVersion = 2;
              mediaType = "application/vnd.docker.distribution.manifest.list.v2+json";
              manifests = [
                {
                  mediaType = "application/vnd.docker.distribution.manifest.v2+json";
                  size = stringLength "${readFile manifests.amd64}";
                  digest = "sha256:${hashFile "sha256" manifests.amd64}";
                  platform.architecture = "amd64";
                  platform.os = "linux";
                }
                {
                  mediaType = "application/vnd.docker.distribution.manifest.v2+json";
                  size = stringLength "${readFile manifests.arm64}";
                  digest = "sha256:${hashFile "sha256" manifests.arm64}";
                  platform.architecture = "arm64";
                  platform.os = "linux";
                }
              ];
            });

            dirs.amd64 = buildImageDir amd64;
            dirs.arm64 = buildImageDir arm64;

            dir =
              pkgs.runCommand "${name}-oci-${base}-dir" {}
              ''
                mkdir -p $out
                cp ${dirs.amd64}/* $out/
                mv $out/manifest.json $out/${hashFile "sha256" manifests.amd64}.manifest.json
                rm -f $out/version
                cp ${dirs.arm64}/* $out/
                mv $out/manifest.json $out/${hashFile "sha256" manifests.arm64}.manifest.json
                rm -f $out/version
                cp ${manifest} $out/manifest.json
              '';
          in
            pkgs.runCommand "${name}-oci-${base}"
            {
              nativeBuildInputs = [pkgs.skopeo];
            }
            ''
              skopeo copy --all --insecure-policy --tmpdir="$(mktemp -d)" dir:${dir} "oci-archive:$out:${name}:${base}"
            '';

          wash-oci-debian = buildMultiArchImage {
            name = "wash";
            base = "debian";
            amd64 = wash-x86_64-unknown-linux-musl-oci-debian;
            arm64 = wash-aarch64-unknown-linux-musl-oci-debian;
          };

          wash-oci-wolfi = buildMultiArchImage {
            name = "wash";
            base = "wolfi";
            amd64 = wash-x86_64-unknown-linux-musl-oci-wolfi;
            arm64 = wash-aarch64-unknown-linux-musl-oci-wolfi;
          };

          wasmcloud-oci-debian = buildMultiArchImage {
            name = "wasmcloud";
            base = "debian";
            amd64 = wasmcloud-x86_64-unknown-linux-musl-oci-debian;
            arm64 = wasmcloud-aarch64-unknown-linux-musl-oci-debian;
          };

          wasmcloud-oci-wolfi = buildMultiArchImage {
            name = "wasmcloud";
            base = "wolfi";
            amd64 = wasmcloud-x86_64-unknown-linux-musl-oci-wolfi;
            arm64 = wasmcloud-aarch64-unknown-linux-musl-oci-wolfi;
          };

          secrets-nats-kv-oci-debian = buildMultiArchImage {
            name = "secrets-nats-kv";
            base = "debian";
            amd64 = secrets-nats-kv-x86_64-unknown-linux-musl-oci-debian;
            arm64 = secrets-nats-kv-aarch64-unknown-linux-musl-oci-debian;
          };

          secrets-nats-kv-oci-wolfi = buildMultiArchImage {
            name = "secrets-nats-kv";
            base = "wolfi";
            amd64 = secrets-nats-kv-x86_64-unknown-linux-musl-oci-wolfi;
            arm64 = secrets-nats-kv-aarch64-unknown-linux-musl-oci-wolfi;
          };
        in
          (concatMapAttrs (_: {packages, ...}: packages) attrs)
          // {
            inherit
              wash-aarch64-unknown-linux-gnu-fhs
              wash-aarch64-unknown-linux-musl-oci-debian
              wash-aarch64-unknown-linux-musl-oci-wolfi
              wash-oci-debian
              wash-oci-wolfi
              wash-riscv64gc-unknown-linux-gnu-fhs
              wash-x86_64-unknown-linux-gnu-fhs
              wash-x86_64-unknown-linux-musl-oci-debian
              wash-x86_64-unknown-linux-musl-oci-wolfi
              wasmcloud-aarch64-unknown-linux-gnu-fhs
              wasmcloud-aarch64-unknown-linux-musl-oci-debian
              wasmcloud-aarch64-unknown-linux-musl-oci-wolfi
              wasmcloud-oci-debian
              wasmcloud-oci-wolfi
              wasmcloud-riscv64gc-unknown-linux-gnu-fhs
              wasmcloud-x86_64-unknown-linux-gnu-fhs
              wasmcloud-x86_64-unknown-linux-musl-oci-debian
              wasmcloud-x86_64-unknown-linux-musl-oci-wolfi
              secrets-nats-kv-oci-debian
              secrets-nats-kv-oci-wolfi
              ;

            default = attrs.wasmcloud.packages.wasmcloud;
            rust = hostRustToolchain;
          };

        withDevShells = {
          devShells,
          pkgs,
          ...
        }:
          extendDerivations {
            buildInputs = [
              pkgs.cargo-audit
              pkgs.minio
              pkgs.redis
              pkgs.skopeo
              pkgs.spire-agent
              pkgs.tinygo
              pkgs.vault-bin
              pkgs.wit-deps
              pkgs.wasm-tools

              pkgs.pkgsUnstable.go
              pkgs.pkgsUnstable.kubectl
              pkgs.pkgsUnstable.kubernetes-helm
              pkgs.pkgsUnstable.nats-server
            ];
          }
          devShells;
      };
}
