{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";

    flake-linter = {
      url = "gitlab:kira-bruneau/flake-linter";
      inputs = {
        flake-utils.follows = "flake-utils";
        nixpkgs.follows = "nixpkgs";
      };
    };

    nixpkgs.url = "nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      flake-utils,
      flake-linter,
      nixpkgs,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        lib = nixpkgs.lib;
        pkgs = nixpkgs.legacyPackages.${system};

        linter = import ./flake-linter.nix { flake-linter-lib = flake-linter.lib.${system}; };

        craneLib = crane.mkLib pkgs;
        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ libcec ];
          C_INCLUDE_PATH = "${pkgs.libcec}/include/libcec";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        default = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });
      in
      {
        checks = {
          inherit default;
          flake-linter = linter.check;
        };

        packages = {
          inherit default;
        };

        apps = {
          inherit (linter) fix;
          default = flake-utils.lib.mkApp { drv = default; };
        };

        devShells = craneLib.devShell { inputsFrom = [ default ]; };
      }
    );
}
