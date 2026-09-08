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
            pkgs.clippy
            pkgs.rustfmt
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

          shellHook = ''
            export RUSTFLAGS="''${RUSTFLAGS:-} -Clink-arg=-fuse-ld=mold"

            # Nothing else couples these two, so a nixpkgs bump would silently
            # reintroduce the browser-revision mismatch this setup exists to fix.
            # Resolved from the repo root, not $PWD, so entering the shell from a
            # subdirectory does not fire a bogus warning.
            _root=$(${pkgs.git}/bin/git rev-parse --show-toplevel 2>/dev/null || true)
            _pj="$_root/crates/diffcore-tauri/ui/package.json"
            if [ -n "$_root" ] && [ -f "$_pj" ]; then
              _pinned=$(${pkgs.jq}/bin/jq -r '.devDependencies."@playwright/test"' "$_pj")
              if [ "$_pinned" != "${pkgs.playwright-driver.version}" ]; then
                echo "warning: @playwright/test is $_pinned but nixpkgs playwright-driver is ${pkgs.playwright-driver.version};" >&2
                echo "         E2E browsers will not match. Repin package.json to ${pkgs.playwright-driver.version}." >&2
              fi
            fi
          '';
        };
      });
    };
}
