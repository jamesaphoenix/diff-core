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
  # `desktop` and `web` are mutually exclusive cargo features (see
  # diffcore-tauri/src/lib.rs), so diffcore-web needs its own derivation.
  webMode ? false,
}:

rustPlatform.buildRustPackage {
  pname = if webMode then "diffcore-web" else "diff-core";
  version = (lib.importTOML ./Cargo.toml).workspace.package.version;
  __structuredAttrs = true;

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  npmDeps = fetchNpmDeps {
    src = ./crates/diffcore-tauri/ui;
    hash = "sha256-AQFay57TfO62s0m1UhFZFlNu5/l/k0zBwmnhhl7+5sw=";
  };
  npmRoot = "crates/diffcore-tauri/ui";

  nativeBuildInputs = [
    pkg-config
    nodejs
    npmHooks.npmConfigHook
  ]
  ++ lib.optionals (!webMode) [
    wrapGAppsHook3
    copyDesktopItems
  ];

  # The web server is a plain axum binary; only the desktop app needs the
  # gtk/webkit stack.
  buildInputs = [
    libgit2
    oniguruma
    openssl
    zlib
  ]
  ++ lib.optionals (!webMode) [
    atk
    cairo
    gdk-pixbuf
    glib
    gtk3
    libsoup_3
    pango
    webkitgtk_4_1
  ];

  env = {
    OPENSSL_NO_VENDOR = true;
    RUSTONIG_SYSTEM_LIBONIG = true;
  };

  preBuild = ''
    npm --prefix crates/diffcore-tauri/ui run build
  '';

  # Desktop app + CLI (production webview assets), or the standalone web
  # server. `--no-default-features` is scoped to diffcore-tauri via `-p` so it
  # does not strip defaults from the rest of the workspace.
  cargoBuildFlags =
    if webMode then
      [
        "-p"
        "diffcore-tauri"
        "--no-default-features"
        "--features"
        "web"
      ]
    else
      [
        "--features"
        "diffcore-tauri/custom-protocol"
      ];

  # Default cargoTestFlags would test the whole workspace with default
  # features, pulling `desktop` back in without gtk/webkit present.
  cargoTestFlags = lib.optionals webMode [
    "-p"
    "diffcore-tauri"
    "--no-default-features"
    "--features"
    "web"
  ];

  desktopItems = lib.optionals (!webMode) [
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

  # Both binaries resolve the UI from ../share/diffcore/ui relative to $out/bin.
  postInstall = ''
    mkdir -p $out/share/diffcore
    cp -r crates/diffcore-tauri/ui/dist $out/share/diffcore/ui
  ''
  + lib.optionalString (!webMode) ''
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
    description =
      if webMode then
        "Semantic diff layer for code review (web server)"
      else
        "Semantic diff layer for code review";
    homepage = "https://github.com/jamesaphoenix/diff-core";
    license = lib.licenses.mit;
    mainProgram = if webMode then "diffcore-web" else "diffcore";
  };
}
