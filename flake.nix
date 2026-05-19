{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks-nix.url = "github:cachix/git-hooks.nix";
    bundlers.url = "github:NixOS/bundlers";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    inputs:
    let
      binName = "vc";
      pname = "vicode";
      meta = {
        description = "coding agent";
        homepage = "https://github.com/anuramat/vicode";
        mainProgram = binName;
        # TODO: longDescription = "";
      };
    in
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks-nix.flakeModule
      ];
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
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
          rustfmt = fenixPkgs.latest.rustfmt;

          nativeBuildInputs = with pkgs; [
            perl # some dependency needs this
          ];

          devTools = with pkgs; [
            just
            fenixPkgs.stable.cargo
            fenixPkgs.stable.clippy
            fenixPkgs.stable.rust-src
            fenixPkgs.stable.rustc
            rustfmt
            cargo-udeps
            cargo-edit
            cargo-expand
            cargo-flamegraph
            cargo-insta
          ];

          mkCraneLib =
            {
              buildPkgs,
              rustStds ? [ ],
            }:
            (inputs.crane.mkLib buildPkgs).overrideToolchain (
              _: # crane passes pkgs, which we don't need
              fenixPkgs.combine (
                [
                  fenixPkgs.stable.rustc
                  fenixPkgs.stable.cargo
                ]
                ++ rustStds
              )
            );

          mkBuild =
            { craneLib, binDeps }:
            let
              commonArgs = {
                inherit nativeBuildInputs;
                src = pkgs.lib.cleanSource ./.;
                strictDeps = true;
              };
              cargoArtifacts = craneLib.buildDepsOnly commonArgs;
              unwrapped = craneLib.buildPackage (
                commonArgs
                // {
                  inherit cargoArtifacts;
                  pname = "${pname}-unwrapped";
                  inherit meta;
                }
              );
              wrapped = pkgs.symlinkJoin {
                inherit pname meta;
                inherit (unwrapped) version;
                paths = [ unwrapped ];
                strictDeps = true;
                nativeBuildInputs = [ pkgs.makeWrapper ];
                postBuild =
                  let
                    binPath = lib.makeBinPath binDeps;
                  in
                  ''
                    wrapProgram $out/bin/${binName} --prefix PATH : ${binPath}
                  '';
              };
            in
            {
              inherit unwrapped wrapped cargoArtifacts;
            };

          linux =
            let
              muslTarget = "x86_64-unknown-linux-musl";
              binDeps = with pkgs; [
                bash
                gnutar
                git
                fuse-overlayfs
                bindfs
                bubblewrap
              ];
              build = mkBuild {
                craneLib = mkCraneLib {
                  buildPkgs = import inputs.nixpkgs {
                    localSystem = system;
                    crossSystem.config = muslTarget;
                  };
                  rustStds = [ fenixPkgs.targets.${muslTarget}.stable.rust-std ];
                };
                inherit binDeps;
              };
              bundled =
                let
                  arxPname = "${pname}-arx";
                  inherit (build.unwrapped) version;
                in
                (inputs.bundlers.bundlers.${system}.toArx build.wrapped).overrideAttrs {
                  name = "${arxPname}-${version}";
                  pname = arxPname;
                };
            in
            {
              packages = {
                "${pname}-arx" = bundled;
                ${pname} = build.wrapped;
                "${pname}-unwrapped" = build.unwrapped;
                "${pname}-deps" = build.cargoArtifacts;
                default = build.wrapped;
              };
              inherit binDeps;
            };

          darwin =
            let
              binDeps = with pkgs; [
                bash
                gnutar
                git
              ];
              build = mkBuild {
                craneLib = mkCraneLib { buildPkgs = pkgs; };
                inherit binDeps;
              };
            in
            {
              packages = {
                ${pname} = build.wrapped;
                "${pname}-unwrapped" = build.unwrapped;
                "${pname}-deps" = build.cargoArtifacts;
                default = build.wrapped;
              };
              inherit binDeps;
            };

          platforms = {
            x86_64-linux = linux;
            aarch64-darwin = darwin;
          };
          platform = platforms.${system};
        in
        {
          devShells.default =
            let
              inherit (config.pre-commit) shellHook;
            in
            pkgs.mkShell {
              inherit shellHook;
              packages = devTools ++ nativeBuildInputs ++ platform.binDeps;
            };
          pre-commit = {
            check.enable = false;
            settings.hooks = {
              treefmt.enable = true;
              cargo-fix = {
                enable = true;
                entry =
                  let
                    cargo-fix = pkgs.writeShellApplication {
                      name = "cargo-fix";
                      text = ''
                        cargo fix "$@" --allow-dirty --allow-staged
                      '';
                    };
                  in
                  lib.getExe cargo-fix;
                pass_filenames = false;
                files = "\\.rs$";
              };
            };
          };
          treefmt = {
            settings.formatter.tombi = {
              command = lib.getExe pkgs.tombi;
              options = [
                "format"
                "--offline"
                "--"
              ];
              includes = [ "*.toml" ];
            };
            programs = {
              nixfmt.enable = true;
              rustfmt = {
                enable = true;
                package = rustfmt;
              };
              just.enable = true;
            };
          };
          packages = platform.packages;
        };
    };
}
