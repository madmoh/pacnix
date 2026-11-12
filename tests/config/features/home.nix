# Home configuration of the user: a dotfile, dconf settings, a user unit, and
# editor settings that the editor rewrites itself. home.packages stays empty.
{ config, lib, ... }:
let
  cfg = config.features.home;
in
{
  options.features.home = {
    enable = lib.mkEnableOption "the home configuration of the user";
    extra = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Add the clock-format dconf key and the hello user unit.";
    };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [ "dconf" ];
    claims.home.home = [ ".config/Code" ".config/dconf" ".local/state" ".cache/nix" ];
    home.users.${config.user} = { lib, ... }: {
      home.file.".config/demo/settings.conf".text = "theme = dark\n";
      dconf.settings."org/gnome/desktop/interface" = {
        color-scheme = "prefer-dark";
      } // lib.optionalAttrs cfg.extra { clock-format = "24h"; };
      systemd.user.services = lib.mkIf cfg.extra {
        hello = {
          Unit.Description = "Hello";
          Service.ExecStart = "/usr/bin/sleep infinity";
          Install.WantedBy = [ "default.target" ];
        };
      };
      # The editor writes its own settings file, so it cannot be a link into the
      # store. Activation merges the declared keys into the file and keeps the
      # keys that the editor added.
      home.activation.editorSettings = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        f="$HOME/.config/Code/User/settings.json"
        run mkdir -p "$(dirname "$f")"
        [ -f "$f" ] || echo '{}' > "$f"
        run /usr/bin/sh -c '/usr/bin/jq -s ".[0] * .[1]" "$1" "$2" > "$1.new" && mv "$1.new" "$1"' sh "$f" ${builtins.toFile "settings.json" (builtins.toJSON {
          "editor.tabSize" = 2;
          "files.trimTrailingWhitespace" = true;
        })}
      '';
    };
  };
}
