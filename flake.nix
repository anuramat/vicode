{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-parts.url = "github:hercules-ci/flake-parts";
    devshell.url = "github:numtide/devshell";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.devshell.flakeModule
        inputs.treefmt-nix.flakeModule
      ];
      systems = [
        "x86_64-linux"
      ];
      perSystem =
        {
          pkgs,
          lib,
          system,
          ...
        }:
        let
          fenixPkgs = inputs.fenix.packages.${system};
          fenix = fenixPkgs.stable;
          craneLib = (inputs.crane.mkLib pkgs).overrideToolchain fenix.toolchain;
          rustfmt = fenixPkgs.latest.rustfmt;
          nativeBuildInputs = [
            pkgs.perl # some dependency needs this
          ];
          runtimeBinDeps = with pkgs; [
            bash
            gnutar
            git
            fuse-overlayfs
            bindfs
            bubblewrap
          ];
          devTools = with pkgs; [
            just
            fenix.cargo
            fenix.clippy
            fenix.rust-src
            fenix.rustc
            rustfmt
            cargo-udeps
            cargo-edit
            cargo-expand
            cargo-flamegraph
          ];
        in
        {
          # NOTE flake-parts module won't work with libraries (`devshells.default = { ... };`) so we use pkgs.mkShell directly
          devShells.default = pkgs.mkShell {
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.openssl ];
            packages = devTools ++ nativeBuildInputs ++ runtimeBinDeps;
          };
          treefmt = {
            programs = {
              nixfmt.enable = true;
              rustfmt = {
                enable = true;
                package = rustfmt;
              };
              just.enable = true;
            };
          };
          packages.default =
            let
              binName = "vc";
            in
            craneLib.buildPackage {
              src = pkgs.lib.cleanSource ./.;
              meta.mainProgram = binName;
              nativeBuildInputs = nativeBuildInputs ++ [
                pkgs.makeWrapper
              ];
              postFixup = ''
                wrapProgram $out/bin/${binName} --prefix PATH : ${lib.makeBinPath runtimeBinDeps}
              '';
            };
        };
    };
}
