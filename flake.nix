{
  description = "eterea - local reading app for saved X posts";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    # Linux only: the package links the Wayland/XKB/Vulkan libraries Rune needs
    # and is verified on x86_64-linux. (x86_64-darwin is no longer supported by
    # current nixpkgs, and the macOS build needs frameworks this flake does not
    # provide.)
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        # Rune's pinned revision needs Rust 1.99 language/library features.
        rust = pkgs.rust-bin.stable."1.99.1".default;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rust;
          rustc = rust;
        };
        linuxBuildInputs = with pkgs; [
          libxkbcommon
          wayland
          vulkan-loader
          fontconfig
          freetype
        ];
        # Libraries Rune/winit/fontconfig dlopen by soname at runtime. The
        # wrapper adds them to LD_LIBRARY_PATH because a Nix binary has no
        # system library search path (the release packager bundles them).
        runtimeLibraries = with pkgs; [
          wayland
          libxkbcommon
          fontconfig
          freetype
          expat
          zlib
          vulkan-loader
          libx11
          libxcursor
          libxrandr
          libxi
          libxext
          libxcb
        ];
      in
      {
        packages.default = rustPlatform.buildRustPackage {
          pname = "eterea";
          version = "0.1.0";
          src = ./.;
          # Dependencies are vendored by `cargo vendor` in a fixed-output
          # derivation. The modern `cargoHash` path is used instead of the
          # legacy `cargoLock.outputHashes` one: that layout cannot represent a
          # crate that appears both on crates.io and from a git source
          # (`tiny-skia-path` 0.11.4 comes from the Rune patch), so building its
          # vendored-deps derivation fails with a symlink permission error.
          cargoHash = "sha256-XyVv8iJo72macB+n4dvMFJDTN0LWhf0CjijfS928Yws=";
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper autoAddDriverRunpath ];
          buildInputs = with pkgs; [ sqlite ffmpeg ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux linuxBuildInputs;
          postInstall = ''
            # Rune/winit/fontconfig dlopen their runtime libraries by soname
            # (Wayland, X11, XKB, fontconfig, Vulkan); a Nix binary has no
            # system library path, so the wrapper supplies them the same way
            # scripts/package-linux-release.sh supplies the release bundle.
            # The driver link holds the host's GPU drivers on NixOS.
            wrapProgram $out/bin/eterea \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.ffmpeg ]} \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibraries}:${pkgs.addDriverRunpath.driverLink}/lib

            # Desktop entry and hicolor icons for the Nix package output. The
            # release tarball produced by scripts/package-linux-release.sh gets
            # the same files from assets/linux/ and docs/assets/brand/.
            install -Dm644 ${./assets/linux/eterea.desktop} $out/share/applications/eterea.desktop
            install -Dm644 ${./docs/assets/brand/icon-16.png} $out/share/icons/hicolor/16x16/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/icon-32.png} $out/share/icons/hicolor/32x32/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/icon-64.png} $out/share/icons/hicolor/64x64/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/icon-128.png} $out/share/icons/hicolor/128x128/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/icon-256.png} $out/share/icons/hicolor/256x256/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/icon-512.png} $out/share/icons/hicolor/512x512/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/icon-1024.png} $out/share/icons/hicolor/1024x1024/apps/eterea.png
            install -Dm644 ${./docs/assets/brand/eterea-icon.svg} $out/share/icons/hicolor/scalable/apps/eterea.svg
          '';

          meta = with pkgs.lib; {
            description = "Local reading app for saved X posts";
            license = licenses.mit;
            platforms = platforms.linux;
            mainProgram = "eterea";
          };
        };
      }
    );
}
