# The explicit package lists of a machine, from fixtures/<machine>.
{ config, lib, ... }:
let
  cfg = config.features.fixtures;
  read = f: lib.filter (l: l != "" && !lib.hasPrefix "#" l) (lib.splitString "\n" (builtins.readFile f));
  list = kind: lib.subtractLists (lib.attrNames cfg.exclude) (read (../fixtures + "/${cfg.machine}/${kind}"));
in
{
  options.features.fixtures = {
    enable = lib.mkEnableOption "the package lists of a machine";
    machine = lib.mkOption { type = lib.types.str; };
    exclude = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "Packages that the VM cannot build or install, with the reason.";
    };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = list "official";
    packages.aur = list "aur";
  };
}
