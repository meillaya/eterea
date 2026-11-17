{
  description = "eterea - local reading app for saved X posts";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
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
      in
      {
        packages.default = rustPlatform.buildRustPackage {
          pname = "eterea";
          version = "0.1.0";
          src = ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "accesskit_macos-0.27.2" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "accesskit_windows-0.35.2" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "parley_engine-0.12.0" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "rune-0.0.1" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "rune-motion-0.1.0" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "rune-staging-0.0.1" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "rune-taffy-0.0.1" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "tiny-skia-path-0.11.4" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "wgpu-hal-30.0.1" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
              "winit-0.30.13" = "sha256-tzLB3Hf4qR4VYsdP3hPF+NolGfygFMMhNcraCdmHtiM=";
            };
          };
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];
          buildInputs = with pkgs; [ sqlite ffmpeg ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux linuxBuildInputs;
          postInstall = ''
            wrapProgram $out/bin/eterea \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.ffmpeg ]}

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
            mainProgram = "eterea";
          };
        };
      }
    );
}
