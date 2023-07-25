{
  description = "stateright model checker";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem
    (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [rust-overlay.overlays.default];
        };
        rust = pkgs.rust-bin.stable.latest.default;
      in {
        formatter = pkgs.alejandra;

        devShell = pkgs.mkShell {
          packages = [
            (rust.override {
              extensions = ["rust-src" "rustfmt"];
            })
            pkgs.cargo-watch
          ];
        };
      }
    );
}
