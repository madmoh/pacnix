{ lib, ... }:
{
  imports = [ ./packages.nix ./files.nix ./units.nix ./state.nix ./arch.nix ];
  options = {
    assertions = lib.mkOption {
      type = lib.types.listOf lib.types.unspecified;
      internal = true;
      default = [ ];
    };
    home.users = lib.mkOption {
      type = lib.types.attrsOf lib.types.deferredModule;
      default = { };
      description = "Home Manager modules, by user name. pacnix activates them after the system step. home.packages must stay empty.";
    };
  };
}
