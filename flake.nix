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
      diffCoreFor = pkgs: pkgs.callPackage ./default.nix { };
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
