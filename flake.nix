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
            pkgs.nodejs
            pkgs.playwright-driver.browsers
          ];
          RUSTC_WRAPPER = "sccache";

          # Playwright browsers come from nixpkgs, never from npm's downloader:
          # the downloaded builds are not patched for NixOS and die on launch.
          # `@playwright/test` in crates/diffcore-tauri/ui/package.json is pinned
          # to exactly this driver's version — a mismatch makes playwright look
          # for a browser revision these browsers do not contain.
          PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
          PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
          PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "1";

          shellHook = ''
            export RUSTFLAGS="''${RUSTFLAGS:-} -Clink-arg=-fuse-ld=mold"
          '';
        };
      });
    };
}
