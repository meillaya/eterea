{ pkgs, lib, ... }:
let
  # Rune draws through wgpu/winit. On Linux the build needs the Wayland and
  # XKB development packages; the window opens through the desktop session's
  # Vulkan driver at runtime. ffmpeg is the video decoder.
  linuxLibraries = with pkgs; [
    libxkbcommon
    wayland
    vulkan-loader
    fontconfig
    freetype
  ];
in
{
  name = "eterea";

  languages.rust.enable = true;
  # Rune's pinned revision uses language features stabilized in Rust 1.99.
  # nixpkgs' default rustc lags behind, so select the same stable line as the
  # workspace toolchain through devenv's rust-overlay channel.
  languages.rust.channel = "stable";
  languages.rust.version = "1.99.1";
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
    git
    ffmpeg
    mesa
  ]
  ++ lib.optionals pkgs.stdenv.isLinux linuxLibraries;

  env = {
    PKG_CONFIG_PATH = lib.makeSearchPath "lib/pkgconfig" (
      [ pkgs.sqlite.dev pkgs.fontconfig.dev pkgs.freetype.dev ]
      ++ lib.optionals pkgs.stdenv.isLinux (map lib.getDev linuxLibraries)
    );
  }
  // lib.optionalAttrs pkgs.stdenv.isLinux {
    LD_LIBRARY_PATH = lib.makeLibraryPath (
      with pkgs;
      [
        sqlite
        libxkbcommon
        wayland
        vulkan-loader
        fontconfig
        freetype
      ]
    );
  };

  enterShell = ''
    echo "eterea dev environment ready"
  '';
}
