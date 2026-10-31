{
  description = "eterea - Lightning-fast Twitter bookmarks manager";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        linuxBuildInputs = with pkgs; [
          webkitgtk_4_1
          gtk3
          libsoup_3
          libayatana-appindicator
        ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "eterea";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ sqlite openssl ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux linuxBuildInputs;
          meta = with pkgs.lib; {
            description = "Local-first X/Twitter bookmark reading room";
            license = licenses.mit;
            mainProgram = "eterea-dioxus";
          };
        };
      }
    );
}
