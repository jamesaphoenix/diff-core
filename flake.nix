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
      diffCoreWebFor = pkgs: pkgs.callPackage ./default.nix { webMode = true; };
    in
    {
      packages = forAllSystems (pkgs: rec {
        diff-core = diffCoreFor pkgs;
        diffcore-web = diffCoreWebFor pkgs;
        default = diff-core;
      });

      # The three modes: `nix run .#cli|desktop|web`
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
          web = {
            type = "app";
            program = "${diffCoreWebFor pkgs}/bin/diffcore-web";
          };
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
