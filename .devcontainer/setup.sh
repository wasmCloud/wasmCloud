#!/bin/sh
set -xe

nix profile install --inputs-from . \
    'nixpkgs#nix-direnv' \
    'nixpkgs#nixd' \
    'nixpkgs#rust-analyzer' \
    'nixpkgs#gcc' \
    '.#rust'

echo 'source "$HOME/.nix-profile/share/nix-direnv/direnvrc"' > "$HOME/.direnvrc"

# Pre-build nix development shell
nix develop -L -c echo nix development environment built
