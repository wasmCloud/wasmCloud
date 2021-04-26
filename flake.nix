{
  description = "wash - WASMCloud Shell";

  inputs = {
    nixpkgs.url = github:NixOS/nixpkgs/nixos-unstable;
    flakeutils.url = "github:numtide/flake-utils";
    naersk.url = "github:nmattia/naersk";
  };

  outputs = { self, nixpkgs, flakeutils, naersk }:
    flakeutils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages."${system}";
        naersk-lib = naersk.lib."${system}";
      in
      rec {
        packages.wash = naersk-lib.buildPackage {
          pname = "wash";
          src = self;
          root = ./.;

          # Workaround for lack of a naersk option to select --bin target.
          # See https://github.com/nmattia/naersk/issues/127
          singleStep = true;
          cargoBuildOptions = (opts: opts ++ ["--bin=wash"]);

          buildInputs = with pkgs; [
            pkgconfig
            clang
            llvmPackages.libclang
          ];
          propagatedBuildInputs = with pkgs; [
            openssl
          ];
          runtimeDependencies = with pkgs; [
            openssl
          ];

          # Allow build step to find libclang.so path.
          LD_LIBRARY_PATH = "${pkgs.llvmPackages.libclang}/lib/";
        };

        defaultPackage = packages.wash;

        apps.wash = flakeutils.lib.mkApp {
          drv = packages.wash;
        };
        defaultApp = apps.wash;

        devShell = pkgs.stdenv.mkDerivation {
          name = "wash";
          src = self;
          buildInputs = with pkgs; [
            pkgconfig
            rustc
            cargo
            clang
            llvmPackages.libclang
          ];
          propagatedBuildInputs = with pkgs; [
            openssl
          ];

          RUST_BACKTRACE = "1";
          # Allow build step to find libclang.so path.
          LD_LIBRARY_PATH = "${pkgs.llvmPackages.libclang}/lib/";
        };
      }
    );
}
