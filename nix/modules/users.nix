{ lib, ... }:
let
  inherit (lib) mkOption types;
in
{
  options.users.users = mkOption {
    default = { };
    description = "User accounts, by name. pacnix creates a missing account with useradd and sets the declared shell, groups and description of each account with usermod. When the declaration of an account that pacnix created goes, pacnix deletes the account with userdel and keeps its home directory. The configuration has no passwords: the apply asks for the password of each account that it creates.";
    type = types.attrsOf (types.submodule ({ name, ... }: {
      options = {
        uid = mkOption {
          type = types.nullOr types.int;
          default = null;
          description = "User ID. pacnix does not change the ID of an existing account, and stops when it is different.";
        };
        group = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Primary group. null: a new account gets a group with its name, and an existing account keeps its group.";
        };
        extraGroups = mkOption {
          type = types.listOf types.str;
          default = [ ];
          description = "Supplementary groups. The account is in these groups and in no other supplementary group.";
        };
        shell = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "/usr/bin/zsh";
          description = "Login shell. Declare its package. null: the default of useradd for a new account, no change for an existing account.";
        };
        home = mkOption {
          type = types.str;
          default = "/home/${name}";
          description = "Home directory of a new account. pacnix does not move the home directory of an existing account.";
        };
        description = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Full name or comment (GECOS field).";
        };
      };
    }));
  };
  options.users.groups = mkOption {
    default = { };
    description = "Groups, by name. pacnix creates a missing group with groupadd. When the declaration of a group that pacnix created goes, pacnix deletes the group with groupdel.";
    type = types.attrsOf (types.submodule {
      options.gid = mkOption {
        type = types.nullOr types.int;
        default = null;
        description = "Group ID of a new group.";
      };
    });
  };
}
