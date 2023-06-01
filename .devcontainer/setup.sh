#!/bin/sh
set -xe

nix profile install --inputs-from . \
    'nixpkgs#nix-direnv' \
    'nixpkgs#rnix-lsp' \
    'nixpkgs#rust-analyzer' \
    'nixpkgs#stdenv.cc.cc' \
    '.#rust'

echo 'source "$HOME/.nix-profile/share/nix-direnv/direnvrc"' > "$HOME/.direnvrc"

# Pre-build nix development shell
nix develop -L -c echo nix development environment built
