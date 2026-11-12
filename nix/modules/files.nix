{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  denied = [ "/usr/" "/nix/" "/home/" "/proc/" "/sys/" "/dev/" "/run/" "/tmp/" "/var/lib/pacman/" "/var/lib/pacnix/" ];
  allowed = path: lib.hasPrefix "/" path && !(lib.hasSuffix "/" path) && (lib.hasPrefix "/usr/local/" path || !(lib.any (p: lib.hasPrefix p path) denied));
in
{
  options.files = mkOption {
    default = { };
    description = "Regular files and symbolic links outside home directories, by absolute path. pacnix writes them with atomic replace.";
    type = types.attrsOf (types.submodule {
      options = {
        text = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Content of the file. Set one of text, source and target.";
        };
        source = mkOption {
          type = types.nullOr types.path;
          default = null;
          description = "File that supplies the content. Set one of text, source and target.";
        };
        target = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "/usr/share/zoneinfo/Europe/Berlin";
          description = "Target of a symbolic link. Set one of text, source and target. Mode, user and group do not apply.";
        };
        mode = mkOption {
          type = types.strMatching "0?[0-7]{3,4}";
          default = "0644";
        };
        user = mkOption {
          type = types.str;
          default = "root";
        };
        group = mkOption {
          type = types.str;
          default = "root";
        };
      };
    });
  };
  options.triggers = mkOption {
    default = { };
    description = "Commands that pacnix runs after a change to a managed file. A path that ends with a slash matches all files below it.";
    example = { mkinitcpio = { paths = [ "/etc/mkinitcpio.conf.d/" ]; command = "mkinitcpio -P"; }; };
    type = types.attrsOf (types.submodule {
      options = {
        paths = mkOption { type = types.listOf types.str; };
        command = mkOption { type = types.str; };
      };
    });
  };
  config.assertions = lib.concatLists (lib.mapAttrsToList (path: f: [
    {
      assertion = lib.count (v: v != null) [ f.text f.source f.target ] == 1;
      message = "files.\"${path}\": set exactly one of text, source and target.";
    }
    {
      assertion = allowed path;
      message = "files.\"${path}\": use an absolute file path outside ${toString denied} (/usr/local is permitted).";
    }
  ]) config.files);
}
