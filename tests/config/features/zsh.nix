# zsh with antidote. antidote writes its generated plugin file next to the
# declared plugin list, so the home configuration links each file into ZDOTDIR.
{ config, lib, ... }:
{
  options.features.zsh.enable = lib.mkEnableOption "zsh with antidote, zoxide and fzf";
  config = lib.mkIf config.features.zsh.enable {
    packages.repo = [ "zsh" "fzf" "zoxide" ];
    packages.aur = [ "zsh-antidote" "trash-d" ];
    home.users.${config.user} = {
      home.file.".zshenv".text = ''
        export ZDOTDIR="$HOME/.config/zsh"
      '';
      xdg.configFile."zsh/.zsh_plugins.txt".text = ''
        zsh-users/zsh-completions
        zsh-users/zsh-autosuggestions
        zsh-users/zsh-syntax-highlighting
      '';
      xdg.configFile."zsh/.zshrc".text = ''
        source /usr/share/zsh-antidote/antidote.zsh
        antidote load
        eval "$(zoxide init zsh)"
        source <(fzf --zsh)
        HISTFILE="$HOME/.local/state/zsh/history"
      '';
    };
    claims.zsh.home = [ ".config/zsh/.zsh_plugins.zsh" ".config/zsh/.zcompdump*" ".cache/antidote" ".local/share/zoxide" ".local/state/zsh" ];
  };
}
