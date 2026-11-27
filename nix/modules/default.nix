{ lib, ... }:
{
  imports = [ ./packages.nix ./files.nix ./units.nix ./state.nix ./arch.nix ./users.nix ./filesystems.nix ./boot.nix ./system.nix ./notes.nix ];
  options = {
    assertions = lib.mkOption {
      type = lib.types.listOf lib.types.unspecified;
      internal = true;
      default = [ ];
    };
    # The warnings of the module system, for example of a renamed option: apply and audit show them.
    warnings = lib.mkOption {
      type = lib.types.listOf lib.types.str;
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
