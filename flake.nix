{
  nixConfig.extra-substituters = [
    "https://wasmcloud.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-public-keys = [
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
  ];

  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = github:rvolosatovs/nixify;
  inputs.nixlib.url = github:nix-community/nixpkgs.lib;
  inputs.wasmcloud-blobstore.flake = false;
  inputs.wasmcloud-blobstore.url = "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces@10f71d127ba11e580ae912f3128761c6d4e02ca4/blobstore/blobstore.smithy";
  inputs.wasmcloud-core.flake = false;
  inputs.wasmcloud-core.url = "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces/core/wasmcloud-core.smithy";
  inputs.wasmcloud-model.flake = false;
  inputs.wasmcloud-model.url = "https://cdn.jsdelivr.net/gh/wasmcloud/interfaces/core/wasmcloud-model.smithy";

  outputs = {
    nixify,
    nixlib,
    wasmcloud-blobstore,
    wasmcloud-core,
    wasmcloud-model,
    ...
  }:
    with nixlib.lib;
    with builtins;
    with nixify.lib; let
      src = filterSource {
        src = ./.;
        exclude = [
          ".archived"
          ".gitignore"
          "awesome-wasmcloud"
          "CODE_OF_CONDUCT.md"
          "CONTRIBUTING.md"
          "CONTRIBUTION_LADDER.md"
          "GOVERNANCE.md"
          "LICENSE"
          "OWNERS"
          "providers/.github"
          "providers/build"
          "README.md"
          "ROADMAP.md"
          "SECURITY.md"
        ];
      };

      excludePaths = [
        "rust-toolchain.toml"
      ];

      rustupToolchain = readTOML ./rust-toolchain.toml;

      doCheck = false; # testing is performed in checks via `nextest`

      clippy.deny = ["warnings"];

      buildOverrides = {pkgs, ...} @ args: {
        buildInputs ? [],
        depsBuildBuild ? [],
        ...
      }: let
        smithyModels = pkgs.stdenv.mkDerivation {
          name = "smithy-models";

          dontUnpack = true;
          installPhase = ''
            install -D ${wasmcloud-blobstore} $out/cdn.jsdelivr.net/blobstore.6e1a8e9635d103b4.smithy
            install -D ${wasmcloud-core} $out/cdn.jsdelivr.net/wasmcloud-core.c781c4623fcaafd5.smithy
            install -D ${wasmcloud-model} $out/cdn.jsdelivr.net/wasmcloud-model.4c523bf5e1147b02.smithy
          '';
        };
      in
        with (args.pkgsCross or pkgs); {
          SMITHY_CACHE = "NO_EXPIRE";

          preBuild =
            ''
              export HOME=$(mktemp -d)
            ''
            + optionalString stdenv.hostPlatform.isLinux ''
              mkdir -p $HOME/.cache
              ln -s ${smithyModels} $HOME/.cache/smithy
            ''
            + optionalString stdenv.hostPlatform.isDarwin ''
              mkdir -p $HOME/Library/Caches
              ln -s ${smithyModels} $HOME/Library/Caches/smithy
            '';

          depsBuildBuild =
            depsBuildBuild
            ++ [
              smithyModels

              pkgs.git
              pkgs.protobuf # build dependency of prost-build v0.9.0
            ];

          buildInputs =
            buildInputs
            ++ optional stdenv.isDarwin libiconv;
        };

      # Attribute set mapping provider names to their attribute set constructor (partially-aplied `mkAttrs`)
      providers = let
        dir = readDir ./providers;
        isProviderDir = name: type: name != "build" && name != ".github" && type == "directory";
        providerDirs = filterAttrs isProviderDir dir;
        providers = attrNames providerDirs;
        mkProvider = name: let
          # parsed `Cargo.toml` file of this provider
          cargoToml = readTOML "${src}/providers/${name}/Cargo.toml";

          # crate name of this provider
          pname = cargoToml.package.name;
        in
          rust.mkAttrs {
            inherit
              buildOverrides
              doCheck
              pname
              rustupToolchain
              ;

            build.packages = [pname];
            clippy =
              clippy
              // {
                packages = [pname];
              };
            doc.packages = [pname];
            test.packages = [pname];

            src = filterSource {
              inherit src;

              exclude = excludePaths;
            };
            version = cargoToml.package.version;

            targets.wasm32-wasi = false;
          };
      in
        genAttrs providers mkProvider;

      prefixAttrs = name: mapAttrs' (k: nameValuePair "${name}-${k}");
      mergeAttrValues = attr: foldr mergeAttrs {} (attrValues attr);
    in
      rust.mkFlake {
        inherit
          buildOverrides
          clippy
          doCheck
          excludePaths
          rustupToolchain
          src
          ;

        targets.armv7-unknown-linux-musleabihf = false;
        targets.wasm32-wasi = false;

        withChecks = {
          checks,
          pkgs,
          ...
        }: let
          providerChecks = mergeAttrValues (mapAttrs (name: mkAttrs: prefixAttrs name (mkAttrs pkgs).checks) providers);
        in
          checks
          // providerChecks;

        withOverlays = {overlays, ...}: let
          providerOverlays = mapAttrs (name: mkAttrs: final: prev: (mkAttrs final).overlay prev) providers;
        in
          overlays
          // providerOverlays;

        withPackages = {
          packages,
          pkgs,
          ...
        }: let
          providerPackages = mergeAttrValues (mapAttrs (name: mkAttrs: (mkAttrs pkgs).packages) providers);
        in
          packages
          // providerPackages;
      };
}
