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
        root = ./.;
        lib = nixpkgs.lib;
        pkgs = nixpkgs.legacyPackages.${system};

        linter = import ./flake-linter.nix { flake-linter-lib = flake-linter.lib.${system}; };

        craneLib = crane.mkLib pkgs;
        commonArgs = {
          src = lib.fileset.toSource {
            root = root;
            fileset = lib.fileset.unions [
              (craneLib.fileset.commonCargoSources root)
              (lib.fileset.maybeMissing ./wayland-protocols)
            ];
          };

          strictDeps = true;

          nativeBuildInputs = with pkgs; [ pkg-config ];

          buildInputs = with pkgs; [
            libcec
            udev
          ];

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
          gamescope = pkgs.fetchFromGitHub {
            owner = "ValveSoftware";
            repo = "gamescope";
            rev = "refs/tags/3.15.14";
            hash = "sha256-/g0/f7WkkS3AouvLQmRaiDbMyVEfikeoOCqqFjmWO0k=";
          };

          mpris-zbus = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "mpris-zbus";
            version = "2.2";

            src = pkgs.fetchFromGitLab {
              domain = "gitlab.freedesktop.org";
              owner = "mpris";
              repo = "mpris-spec";
              rev = "refs/tags/v${finalAttrs.version}";
              hash = "sha256-gkL81/wlSkS5BNZ4BHEtWKyDhQGvwKPvNbljPt6hiQE=";
            };

            nativeBuildInputs = with pkgs; [ zbus-xmlgen ];

            # Remove problematic tp:type attributes
            # https://github.com/dbus2/zbus/issues/255
            postPatch = ''
              find spec -type f -iname '*.xml' -print0 | xargs -0 -n1 sed -i 's/tp:type="[^"]*"//g'
            '';

            dontBuild = true;

            installPhase = ''
              mkdir "$out"
              cd "$out"
              find "$NIX_BUILD_TOP/$sourceRoot/spec" -type f -iname '*.xml' -print0 | xargs -0 -n1 -exec zbus-xmlgen file
            '';
          });

          inherit default;
        };

        apps = {
          inherit (linter) fix;
          default = flake-utils.lib.mkApp { drv = default; };
        };

        devShells.default = craneLib.devShell { inputsFrom = [ default ]; };
      }
    );
}
