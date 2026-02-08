{
  description = "Snapback - restore metadata and captions to Snapchat memory exports";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    naersk = {
      url = "github:nix-community/naersk/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];

      perSystem =
        {
          config,
          self',
          inputs',
          pkgs,
          system,
          ...
        }:
        let
          naersk' = pkgs.callPackage inputs.naersk { };

          runtimeDeps = [
            pkgs.exiftool
            pkgs.ffmpeg-headless
            pkgs.unzip
          ];

          snapback-unwrapped = naersk'.buildPackage {
            src = ./.;
          };

          snapback = pkgs.symlinkJoin {
            name = "snapback";
            paths = [ snapback-unwrapped ];
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postBuild = ''
              wrapProgram $out/bin/snapback \
                --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
            '';
          };
        in
        {
          packages = {
            default = snapback;
            unwrapped = snapback-unwrapped;
          };

          devShells.default = pkgs.mkShell {
            buildInputs = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.rustPackages.clippy
            ]
            ++ runtimeDeps;

            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
          };

          formatter = pkgs.nixfmt-rfc-style;
        };

      flake = { };
    };
}
