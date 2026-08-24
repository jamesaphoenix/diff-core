{
  description = "Semantic diff layer for code review";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # Linux-only: the package hard-depends on webkitgtk/gtk3.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      # libgit2-sys 0.18.7 requires libgit2 >= 1.9.6; nixpkgs has 1.9.4, which
      # silently falls back to building the bundled C copy on every clean build.
      diffCoreFor =
        pkgs:
        pkgs.callPackage ./default.nix {
          libgit2 = pkgs.libgit2.overrideAttrs (old: {
            version = "1.9.6";
            src = pkgs.fetchFromGitHub {
              owner = "libgit2";
              repo = "libgit2";
              rev = "v1.9.6";
              hash = "sha256-ogowkZrw9MG4pcgXHzzNX5fm1Z8L2tNW8MDfL4ySJyY=";
            };
          });
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        diff-core = diffCoreFor pkgs;
        default = diff-core;
      });

      # The two modes: `nix run .#cli|desktop`
      apps = forAllSystems (
        pkgs:
        let
          pkg = diffCoreFor pkgs;
          app = program: {
            type = "app";
            program = "${pkg}/bin/${program}";
          };
        in
        rec {
          cli = app "diffcore";
          desktop = app "diffcore-tauri";
          default = desktop;
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ (diffCoreFor pkgs) ];
          packages = [
            pkgs.sccache
            pkgs.mold
          ];
          RUSTC_WRAPPER = "sccache";
          shellHook = ''
            export RUSTFLAGS="''${RUSTFLAGS:-} -Clink-arg=-fuse-ld=mold"
          '';
        };
      });
    };
}
