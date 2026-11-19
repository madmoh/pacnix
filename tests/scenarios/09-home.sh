#!/usr/bin/env bash
# Scenario 9: Home Manager activation over an unmanaged dotfile, which pacnix
# keeps as the original, also when the first activation stops before it links
# the files. A changed link target gets the declared file again, and the
# removal of the declaration gives the original back. Removal of a dconf key
# and a user unit, and editor settings that the editor rewrites.
source "$(dirname "$0")/../lib.sh"
fresh $h
on $h 'mkdir -p ~/.config/demo && echo mine >~/.config/demo/settings.conf && touch ~/.fail-activation'
scenario $h '{ features.home.enable = true; }'
step "the first activation stops before it links the files"
out=$(plan $h)
has "$out" 'change home file /home/arch/.config/demo/settings.conf keep the original as /home/arch/.config/demo/settings.conf.pacnix-hm-backup' || { echo "$out"; fail "the plan does not keep the original dotfile"; }
apply $h >/dev/null && fail "the activation did not stop"
check $h 'test "$(cat ~/.config/demo/settings.conf.pacnix-hm-backup)" = mine' "the original dotfile is not kept"
step "activate the home configuration"
on $h 'rm ~/.fail-activation'
apply $h || fail "apply failed"
check $h 'grep -qx "theme = dark" ~/.config/demo/settings.conf && test "$(cat ~/.config/demo/settings.conf.pacnix-hm-backup)" = mine' "the dotfile or its original is wrong"
check $h 'test "$(dconf read /org/gnome/desktop/interface/clock-format)" = "'"'24h'"'"' "the dconf key is not set"
check $h 'systemctl --user is-active hello.service' "the user unit is not running"
check $h 'test "$(jq -r ".\"editor.tabSize\"" ~/.config/Code/User/settings.json)" = 2' "the editor settings are not merged"
check $h 'test ! -L ~/.config/Code/User/settings.json' "the editor settings file is a link"
empty_plan $h

step "an application replaces the dotfile, then the configuration changes"
on $h 'f=~/.config/demo/settings.conf; rm $f && echo changed >$f'
scenario $h '{ features.home.enable = true; home.users.arch.home.sessionVariables.PACNIX_TEST = "1"; }'
out=$(plan $h)
has "$out" 'change home file /home/arch/.config/demo/settings.conf (it differs from the declaration)' || { echo "$out"; fail "the plan does not replace the changed dotfile"; }
apply $h >/dev/null || fail "apply failed"
check $h 'grep -qx "theme = dark" ~/.config/demo/settings.conf && test "$(cat ~/.config/demo/settings.conf.pacnix-hm-backup)" = mine' "the changed dotfile is not replaced, or the original changed"
step "the editor rewrites its settings, then the configuration changes"
on $h 'f=~/.config/Code/User/settings.json; jq ".\"editor.tabSize\" = 8 | .\"editor.fontSize\" = 20" $f > $f.new && mv $f.new $f'
scenario $h '{ features.home.enable = true; features.home.extra = false; }'
apply $h || fail "apply failed"
check $h 'test "$(jq -r ".\"editor.tabSize\"" ~/.config/Code/User/settings.json)" = 2' "the declared editor key is not restored"
check $h 'test "$(jq -r ".\"editor.fontSize\"" ~/.config/Code/User/settings.json)" = 20' "the key of the editor is lost"
check $h 'test "$(cat ~/.config/demo/settings.conf)" = mine && test ! -e ~/.config/demo/settings.conf.pacnix-hm-backup' "the original dotfile is not back"
check $h 'test -z "$(dconf read /org/gnome/desktop/interface/clock-format)"' "the removed dconf key stays set"
check $h 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the kept dconf key is lost"
refute $h 'systemctl --user is-active hello.service' "the removed user unit still runs"
refute $h 'test -e ~/.config/systemd/user/hello.service' "the removed user unit file remains"
empty_plan $h

step "home.packages must stay empty"
scenario $h '{ features.home.enable = true; home.users.arch = { pkgs, ... }: { home.packages = [ pkgs.hello ]; }; }'
out=$(plan $h) && fail "the plan accepts home.packages"
has "$out" 'Home Manager installs these packages from Nix: hello' || { echo "$out"; fail "wrong error for home.packages"; }
pass
