{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    devshell.url = "github:numtide/devshell";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    rust-nightly.url = "github:oxalica/rust-overlay";
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
        { pkgs, system, ... }:
        let
          rust-nightly = inputs.rust-nightly.packages.${system}.rust-nightly;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust-nightly;
            rustc = rust-nightly;
          };
          rustfmt = pkgs.writeShellScriptBin "rustfmt" ''
            exec ${rust-nightly}/bin/rustfmt "$@"
          '';

          # TODO add these to the nix package as well
          runtimeDeps = with pkgs; [
            git
            fuse-overlayfs
            bubblewrap
          ];
        in
        {

          # NOTE that flake-parts module won't work with libraries: devshells.default = {
          # so we use pkgs.mkShell directly
          devShells.default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                just

                # cargo stuff
                # cargo
                cargo-udeps
                cargo-edit
                # clippy
                rust-analyzer
                # rustc
                rust-nightly
                # rustfmt
                cargo-expand
                cargo-flamegraph

                # build deps
                pkg-config
                openssl
              ]
              ++ runtimeDeps;
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
              crate = pkgs.lib.importTOML ./Cargo.toml;
            in
            rustPlatform.buildRustPackage {
              preferLocalBuild = true;
              allowSubstitutes = false;
              pname = crate.package.name;
              version = crate.package.version;
              src = pkgs.lib.cleanSource ./.;
              cargoLock = {
                lockFile = ./Cargo.lock;
                outputHashes."async-openai-0.33.0" = "sha256-lvaHXQ41f2ci8aX2fKZ19p5L7wTqjUn6xh/XzZZ9oL0=";
              };
              nativeBuildInputs = with pkgs; [
                pkg-config
              ];
              buildInputs = with pkgs; [
                openssl
              ];
              meta.mainProgram = "vc";
            };
        };
    };
}
