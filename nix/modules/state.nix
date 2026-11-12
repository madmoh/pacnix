{ lib, ... }:
let
  inherit (lib) mkOption types;
in
{
  options.claims = mkOption {
    default = { };
    description = "Paths that a feature creates at run time, by feature name. The audit does not report claimed paths. Patterns are globs. Home patterns are relative to each home directory.";
    type = types.attrsOf (types.submodule {
      options = {
        system = mkOption { type = types.listOf types.str; default = [ ]; };
        home = mkOption { type = types.listOf types.str; default = [ ]; };
      };
    });
  };
  options.audit.ignore = mkOption {
    type = types.listOf types.str;
    default = [ ];
    description = "Glob patterns of paths that the audit does not report.";
  };
}
