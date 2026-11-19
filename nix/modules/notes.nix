{ config, lib, ... }:
let
  inherit (lib) mkOption types;
in
{
  options.installNotes = mkOption {
    default = { };
    description = "Manual steps that apply shows at its end, by name. Each feature can add its own notes. A note without a check shows at the end of the first apply that completes all steps, and again when its message changes or when the note goes and comes back. A note with a check shows while the check exits with status 1. Plan and audit do not show the notes and do not run the checks. A note is not a change of the system.";
    type = types.attrsOf (types.submodule {
      options = {
        message = mkOption {
          type = types.str;
          description = "The step that the user must do.";
        };
        check = mkOption {
          default = null;
          description = "A command that each apply runs after the changes, also when nothing changes. It runs as the user with setpriv, without a shell, without input, with HOME, USER, LOGNAME and PATH, with XDG_RUNTIME_DIR and the session bus when the user has them, and with a time limit of 5 s. Exit status 0: the step is done, and the note does not show. Exit status 1: the step is not done, and the note shows. Each other result shows one line with the cause, and apply continues. pacnix does not show the output of the command. null: no check.";
          type = types.nullOr (types.submodule {
            options = {
              user = mkOption { type = types.str; description = "The user that runs the command. It cannot be root or have the UID 0."; };
              command = mkOption { type = types.listOf types.str; description = "The program, with an absolute path, and its arguments. No shell runs it."; };
            };
          });
        };
      };
    });
  };
  config.assertions = lib.concatLists (lib.mapAttrsToList (name: n: [
    { assertion = lib.trim n.message != ""; message = "installNotes.${name}.message: the message is empty"; }
  ] ++ lib.optionals (n.check != null) [
    { assertion = n.check.command != [ ] && lib.hasPrefix "/" (lib.head n.check.command); message = "installNotes.${name}.check.command: give the program with an absolute path as the first item"; }
    { assertion = n.check.user != "" && n.check.user != "root"; message = "installNotes.${name}.check.user: give a user that is not root"; }
  ]) config.installNotes);
}
