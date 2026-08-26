{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook3,
  nodejs,
  npmHooks,
  fetchNpmDeps,
  copyDesktopItems,
  makeDesktopItem,
  atk,
  cairo,
  gdk-pixbuf,
  git,
  glib,
  gtk3,
  libgit2,
  libsoup_3,
  oniguruma,
  openssl,
  pango,
  webkitgtk_4_1,
  zlib,
}:

rustPlatform.buildRustPackage {
  pname = "diff-core";
  version = (lib.importTOML ./Cargo.toml).workspace.package.version;
  __structuredAttrs = true;

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  npmDeps = fetchNpmDeps {
    src = ./crates/diffcore-tauri/ui;
    hash = "sha256-RdhH63p7Cb9IjDx3SmMAtZRL+py54BMYoLHrarNlmS8=";
  };
  npmRoot = "crates/diffcore-tauri/ui";

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook3
    nodejs
    npmHooks.npmConfigHook
    copyDesktopItems
  ];

  buildInputs = [
    atk
    cairo
    gdk-pixbuf
    glib
    gtk3
    libgit2
    libsoup_3
    oniguruma
    openssl
    pango
    webkitgtk_4_1
    zlib
  ];

  env = {
    OPENSSL_NO_VENDOR = true;
    RUSTONIG_SYSTEM_LIBONIG = true;
  };

  preBuild = ''
    npm --prefix crates/diffcore-tauri/ui run build
  '';

  # Desktop app + CLI (production webview assets). The `web` feature is
  # mutually exclusive with `desktop` upstream, so diffcore-web needs its
  # own derivation.
  cargoBuildFlags = [
    "--features"
    "diffcore-tauri/custom-protocol"
  ];

  desktopItems = [
    (makeDesktopItem {
      name = "diffcore-tauri";
      exec = "diffcore-tauri";
      icon = "diffcore-tauri";
      desktopName = "Diffcore";
      genericName = "Semantic Diff Viewer";
      comment = "Semantic diff layer for code review";
      categories = [
        "Development"
        "RevisionControl"
      ];
      startupWMClass = "diffcore-tauri";
    })
  ];

  postInstall = ''
    mkdir -p $out/share/diffcore
    cp -r crates/diffcore-tauri/ui/dist $out/share/diffcore/ui
    for size in 32 64 128; do
      install -Dm444 crates/diffcore-tauri/icons/''${size}x''${size}.png \
        $out/share/icons/hicolor/''${size}x''${size}/apps/diffcore-tauri.png
    done
    install -Dm444 crates/diffcore-tauri/icons/128x128@2x.png \
      $out/share/icons/hicolor/256x256/apps/diffcore-tauri.png
    install -Dm444 crates/diffcore-tauri/icons/icon.png \
      $out/share/icons/hicolor/512x512/apps/diffcore-tauri.png
  '';

  nativeCheckInputs = [ git ];

  preCheck = ''
    export HOME=$(mktemp -d)
    export GIT_AUTHOR_NAME=nixbld
    export GIT_AUTHOR_EMAIL=nixbld@localhost
    export GIT_COMMITTER_NAME=nixbld
    export GIT_COMMITTER_EMAIL=nixbld@localhost
  '';

  meta = {
    description = "Semantic diff layer for code review";
    homepage = "https://github.com/jamesaphoenix/diff-core";
    license = lib.licenses.mit;
    mainProgram = "diffcore";
  };
}
