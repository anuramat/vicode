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
      targets = {
        "x86_64-linux" = "x86_64-unknown-linux-musl";
        "aarch64-darwin" = "aarch64-apple-darwin";
      };
    in
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks-nix.flakeModule
      ];
      systems = builtins.attrNames targets;
      perSystem =
        {
          pkgs,
          lib,
          system,
          config,
          ...
        }:
        let
          target = targets.${system};
          xcPkgs = import inputs.nixpkgs {
            localSystem = system;
            crossSystem.config = target;
          };
          fenixPkgs = inputs.fenix.packages.${system};

          craneLib = (inputs.crane.mkLib xcPkgs).overrideToolchain (
            _: # crane passes pkgs, which we don't need
            fenixPkgs.combine [
              fenixPkgs.stable.rustc
              fenixPkgs.stable.cargo
              fenixPkgs.targets.${target}.stable.rust-std
            ]
          );

          rustfmt = fenixPkgs.latest.rustfmt;

          nativeBuildInputs = with pkgs; [
            perl # some dependency needs this
          ];

          # TODO drop on darwin, ideally only the missing ones
          binDeps =
            p: with p; [
              bash
              gnutar
              git
              fuse-overlayfs
              bindfs
              bubblewrap
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
          ];
        in
        {
          devShells.default =
            let
              inherit (config.pre-commit) shellHook;
            in
            pkgs.mkShell {
              inherit shellHook;
              packages = devTools ++ nativeBuildInputs ++ (binDeps pkgs);
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
              unwrapped = craneLib.buildPackage {
                pname = "${pname}-unwrapped";
                inherit meta nativeBuildInputs;
                src = pkgs.lib.cleanSource ./.;
                strictDeps = true;
                # TEST if these work on darwin
                CARGO_BUILD_TARGET = target;
                CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
              };
              wrapped = pkgs.symlinkJoin {
                inherit pname meta;
                inherit (unwrapped) version;
                paths = [ unwrapped ];
                nativeBuildInputs = [ xcPkgs.buildPackages.makeWrapper ];
                postBuild =
                  let
                    # HACK we should be using `binDeps xcPkgs`, but then we're rebuilding universe with musl
                    # NOTE essentially xcPkgs.buildPackages = pkgs, but this is clearer in intent (?)
                    binPath = lib.makeBinPath (binDeps xcPkgs.buildPackages);
                  in
                  ''
                    wrapProgram $out/bin/${binName} --prefix PATH : ${binPath}
                  '';
              };
              bundled =
                let
                  arxPname = "${pname}-arx";
                  inherit (unwrapped) version;
                in
                (inputs.bundlers.bundlers.${system}.toArx wrapped).overrideAttrs {
                  name = "${arxPname}-${version}";
                  pname = arxPname;
                };
              packages =
                map (x: lib.nameValuePair (x.pname) x) [
                  bundled
                  wrapped
                  unwrapped
                ]
                |> lib.listToAttrs;
            in
            packages
            // {
              default = wrapped;
            };
        };
    };
}
