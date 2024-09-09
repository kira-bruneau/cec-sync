{ flake-linter-lib }:

let
  paths = flake-linter-lib.partitionToAttrs flake-linter-lib.commonPaths (
    flake-linter-lib.walkFlake ./.
  );
in
flake-linter-lib.makeFlakeLinter {
  root = ./.;

  settings = {
    markdownlint = {
      paths = paths.markdown;
      settings = {
        line-length.code_block_line_length = 100;
        no-duplicate-heading.siblings_only = true;
      };
    };

    nixf-tidy-fix.paths = paths.nix;

    nixfmt-rfc-style.paths = paths.nix;

    rustfmt = {
      paths = paths.rust;
      settings = {
        inherit ((builtins.fromTOML (builtins.readFile ./Cargo.toml)).package) edition;
      };
    };
  };
}
