{
  description = "stateright model checker";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = import nixpkgs
            {
              overlays = [ rust-overlay.overlay ];
              system = system;
            };
          lib = pkgs.lib;
          rust = pkgs.rust-bin.stable.latest.default;
          makeCargoNix = release: import ./Cargo.nix {
            inherit pkgs release;
          };
          cargoNix = makeCargoNix true;
          debugCargoNix = makeCargoNix false;
        in
        rec
        {
          packages =
            lib.attrsets.mapAttrs (name: value: value.build) cargoNix.workspaceMembers;

          checks = lib.attrsets.mapAttrs
            (name: value: value.build.override { runTests = true; })
            debugCargoNix.workspaceMembers;

          devShell = pkgs.mkShell {
            buildInputs = with pkgs;[
              (rust.override {
                extensions = [ "rust-src" "rustfmt" ];
                targets = [ "x86_64-unknown-linux-musl" ];
              })
              cargo-edit
              crate2nix

              rnix-lsp
              nixpkgs-fmt
            ];
          };
        }
      );
}
