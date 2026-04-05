{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks-nix.url = "github:cachix/git-hooks.nix";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks-nix.flakeModule
      ];
      systems = [
        "x86_64-linux"
      ];
      perSystem =
        {
          pkgs,
          lib,
          system,
          config,
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
          devShells.default =
            let
              inherit (config.pre-commit) shellHook;
            in
            pkgs.mkShell {
              inherit shellHook;
              packages = devTools ++ nativeBuildInputs ++ runtimeBinDeps;
            };
          pre-commit.settings.hooks = {
            treefmt.enable = true;
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
          packages =
            let
              binName = "vc";
              meta = {
                description = "coding agent";
                homepage = "https://github.com/anuramat/vicode";
                mainProgram = binName;
                # TODO: longDescription = "";
              };
              vicode-unwrapped = craneLib.buildPackage {
                src = pkgs.lib.cleanSource ./.;
                inherit meta nativeBuildInputs;
              };
              vicode = pkgs.symlinkJoin {
                name = binName;
                paths = [ vicode-unwrapped ];
                nativeBuildInputs = [ pkgs.makeWrapper ];
                postBuild = ''
                  wrapProgram $out/bin/${binName} --prefix PATH : ${lib.makeBinPath runtimeBinDeps}
                '';
                inherit meta;
              };
            in
            {
              inherit
                vicode
                vicode-unwrapped
                ;
              default = vicode;
            };
        };
    };
}
