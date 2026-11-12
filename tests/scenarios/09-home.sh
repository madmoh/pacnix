#!/usr/bin/env bash
# Scenario 9: Home Manager activation, removal of a dconf key and a user unit,
# and editor settings that the editor rewrites.
source "$(dirname "$0")/../lib.sh"
fresh $h
scenario $h '{ features.home.enable = true; }'
step "activate the home configuration"
apply $h || fail "apply failed"
check $h 'grep -qx "theme = dark" ~/.config/demo/settings.conf' "the dotfile is missing"
check $h 'test "$(dconf read /org/gnome/desktop/interface/clock-format)" = "'"'24h'"'"' "the dconf key is not set"
check $h 'systemctl --user is-active hello.service' "the user unit is not running"
check $h 'test "$(jq -r ".\"editor.tabSize\"" ~/.config/Code/User/settings.json)" = 2' "the editor settings are not merged"
check $h 'test ! -L ~/.config/Code/User/settings.json' "the editor settings file is a link"
empty_plan $h

step "the editor rewrites its settings, then the configuration changes"
on $h 'f=~/.config/Code/User/settings.json; jq ".\"editor.tabSize\" = 8 | .\"editor.fontSize\" = 20" $f > $f.new && mv $f.new $f'
scenario $h '{ features.home.enable = true; features.home.extra = false; }'
apply $h || fail "apply failed"
check $h 'test "$(jq -r ".\"editor.tabSize\"" ~/.config/Code/User/settings.json)" = 2' "the declared editor key is not restored"
check $h 'test "$(jq -r ".\"editor.fontSize\"" ~/.config/Code/User/settings.json)" = 20' "the key of the editor is lost"
check $h 'test -z "$(dconf read /org/gnome/desktop/interface/clock-format)"' "the removed dconf key stays set"
check $h 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the kept dconf key is lost"
refute $h 'systemctl --user is-active hello.service' "the removed user unit still runs"
refute $h 'test -e ~/.config/systemd/user/hello.service' "the removed user unit file remains"
empty_plan $h

step "home.packages must stay empty"
scenario $h '{ features.home.enable = true; home.users.arch = { pkgs, ... }: { home.packages = [ pkgs.hello ]; }; }'
out=$(plan $h) && fail "the plan accepts home.packages"
grep -q 'home.packages must stay empty' <<<"$out" || { echo "$out"; fail "wrong error for home.packages"; }
pass
