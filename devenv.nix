{ pkgs, lib, ... }:
let
  # Native libraries the Dioxus desktop window needs on Linux. The GTK stack
  # pulls its own Pango, Cairo, GdkPixbuf, and GLib dependencies, so only the
  # top-level libraries are listed here.
  linuxLibraries = with pkgs; [
    webkitgtk_4_1
    gtk3
    libsoup_3
    librsvg
    libayatana-appindicator
  ];
in
{
  name = "eterea";

  languages.rust.enable = true;
  languages.rust.components = [
    "rustc"
    "cargo"
    "clippy"
    "rustfmt"
    "rust-analyzer"
  ];

  packages = with pkgs; [
    pkg-config
    sqlite
    openssl
    git
  ]
  ++ lib.optionals pkgs.stdenv.isLinux linuxLibraries;

  env = {
    PKG_CONFIG_PATH = lib.makeSearchPath "lib/pkgconfig" (
      [ pkgs.sqlite.dev pkgs.openssl.dev ]
      ++ lib.optionals pkgs.stdenv.isLinux (map lib.getDev linuxLibraries)
    );
  }
  // lib.optionalAttrs pkgs.stdenv.isLinux {
    LD_LIBRARY_PATH = lib.makeLibraryPath (
      with pkgs;
      [
        sqlite
        openssl
        webkitgtk_4_1
        gtk3
        libsoup_3
        libayatana-appindicator
      ]
    );
  };

  enterShell = ''
    echo "Eterea dev environment ready"
  '';
}
