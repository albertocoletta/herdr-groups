{
  description = "herdr-groups: group herdr's Agents panel by git repo";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachSystem [ "aarch64-darwin" "aarch64-linux" "x86_64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        lib = pkgs.lib;
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "herdr-groups";
          version = (lib.importTOML ./Cargo.toml).package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          nativeCheckInputs = [ pkgs.git ];
          postInstall = ''
            cp herdr-plugin.toml $out/
            wrapProgram $out/bin/herdr-groups --prefix PATH : ${lib.makeBinPath [ pkgs.git ]}
          '';
        };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.git
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
}
