{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  absoluteCommand = command: command != [ ] && lib.hasPrefix "/" (lib.head command);
in
{
  options.installNotes = mkOption {
    default = { };
    description = "Setup steps that apply checks after activation, by name. Each step can show a manual instruction or run an action. Plan and audit do not run the steps.";
    type = types.attrsOf (types.submodule {
      options = {
        message = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "The instruction for a manual step. Without a check, it shows once after the prerequisites pass and apply succeeds. It shows again if the message changes or the step is removed and added again.";
        };
        after = mkOption {
          type = types.listOf types.str;
          default = [ ];
          description = "Names of steps whose checks must exit with status 0 before this step can run.";
        };
        action = mkOption {
          type = types.nullOr (types.listOf types.str);
          default = null;
          description = "An absolute program path and arguments. Apply runs the action as check.user when the check exits with status 1. The action has no input and must finish within 120 seconds.";
        };
        check = mkOption {
          default = null;
          description = "A read-only command. Each apply runs the check after activation when the prerequisites pass, even if the configuration has no changes. It has no input and runs as the specified user with HOME, USER, LOGNAME and PATH. It gets XDG_RUNTIME_DIR and the session bus when available. The time limit is 5 seconds. Exit status 0 marks the step complete. Exit status 1 shows the manual instruction or starts the action. Other results block dependent steps. Apply checks the step again after a successful action.";
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
    { assertion = if n.action == null then n.message != null && lib.trim n.message != "" else n.message == null; message = "installNotes.${name}.message: give a message for a manual step only"; }
  ] ++ lib.optionals (n.action != null) [
    { assertion = n.check != null; message = "installNotes.${name}.action: give a check for the action"; }
    { assertion = absoluteCommand n.action; message = "installNotes.${name}.action: give the program with an absolute path as the first item"; }
  ] ++ lib.optionals (n.check != null) [
    { assertion = absoluteCommand n.check.command; message = "installNotes.${name}.check.command: give the program with an absolute path as the first item"; }
    { assertion = n.check.user != "" && n.check.user != "root"; message = "installNotes.${name}.check.user: give a user that is not root"; }
  ] ++ map (prerequisite: {
    assertion = prerequisite != name && (config.installNotes.${prerequisite}.check or null) != null;
    message = "installNotes.${name}.after: give the name of another step with a check. Check ${prerequisite}.";
  }) n.after) config.installNotes);
}
