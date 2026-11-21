#!/usr/bin/env bash
# Scenario 9: Home Manager activation over an unmanaged dotfile, which pacnix
# keeps as the original, also when the first activation stops before it links
# the files. A changed link target gets the declared file again, and the
# removal of the declaration gives the original back. Removal of a dconf key
# and a user unit, and editor settings that the editor rewrites. The keys of a
# named dconf database and of the default database go back to their defaults
# also when the last declaration of the database goes, and the keys that the
# user wrote stay. An activation that stops before the reset keeps the keys,
# and the next apply resets them. A key that two databases on the same file
# declare stays when one of them goes.
source "$(dirname "$0")/../lib.sh"
fresh $h
on $h 'mkdir -p ~/.config/demo && echo mine >~/.config/demo/settings.conf && touch ~/.fail-activation'
scenario $h '{ features.home.enable = true; }'
step "the first activation stops before it links the files"
out=$(plan $h)
has "$out" 'Changing home file /home/arch/.config/demo/settings.conf (pacnix keeps the original as .config/demo/settings.conf.pacnix-hm-backup)' || { echo "$out"; fail "the plan does not keep the original dotfile"; }
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
has "$out" 'Changing home file /home/arch/.config/demo/settings.conf (it differs from the declaration)' || { echo "$out"; fail "the plan does not replace the changed dotfile"; }
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

step "the keys of a dconf database go when its last declaration goes"
# ~/.fail-dconf stops the activation before the reset of the keys of a removed database.
user() { echo "home.users.arch = { lib, ... }: { home.activation.failDconf = lib.hm.dag.entryBefore [ \"pacnixDconfCleanup\" ] ''[ ! -e \"\$HOME/.fail-dconf\" ] || exit 1''; $1 };"; }
base='features.home.enable = true; features.home.extra = false;'
work='dconf.databases.work."org/demo".a = "1";'
shared='dconf.databases.user."org/gnome/desktop/interface".color-scheme = "prefer-dark";'
on $h 'printf "user-db:work\n" >/tmp/work.profile'
wread() { on $h "DCONF_PROFILE=/tmp/work.profile dconf read /org/demo/$1"; }
scheme() { on $h 'dconf read /org/gnome/desktop/interface/color-scheme'; }
scenario $h "{ $base $(user "$work dconf.databases.work.\"org/demo\".b = \"2\";") }"
apply $h >/dev/null || fail "apply failed"
on $h 'DCONF_PROFILE=/tmp/work.profile dconf write /org/demo/mine "'"'m'"'" && dconf write /org/gnome/desktop/interface/font-name "'"'Mine 10'"'"'
[[ $(wread a) == "'1'" && $(wread b) == "'2'" ]] || fail "the keys of the work database are not set"
scenario $h "{ $base $(user "$work") }"
apply $h >/dev/null || fail "apply failed"
[[ $(wread a) == "'1'" && -z $(wread b) ]] || fail "a removed key of the work database stays, or a kept key goes"
on $h 'touch ~/.fail-dconf'
scenario $h "{ $base $(user "") }"
apply $h >/dev/null && fail "the activation did not stop"
[[ $(wread a) == "'1'" ]] || fail "the activation that stopped before the reset changed the work database"
on $h 'rm ~/.fail-dconf'
apply $h >/dev/null || fail "apply failed"
[[ -z $(wread a) ]] || fail "a key of the work database stays after its last declaration goes"
[[ $(wread mine) == "'m'" && $(scheme) == "'prefer-dark'" ]] || fail "the key that the user wrote in the work database or a key of the default database is lost"
step "a shared key stays when one of the two databases that declare it goes"
scenario $h "{ $base $(user "$shared") }"
apply $h >/dev/null || fail "apply failed"
scenario $h "{ $base $(user "") }"
apply $h >/dev/null || fail "apply failed"
[[ $(scheme) == "'prefer-dark'" ]] || fail "the key of the default database is lost after the user database goes"
scenario $h "{ $base $(user "$shared") }"
apply $h >/dev/null || fail "apply failed"
scenario $h "{ $base $(user "$shared dconf.settings = lib.mkForce { };") }"
apply $h >/dev/null || fail "apply failed"
[[ $(scheme) == "'prefer-dark'" ]] || fail "the key of the user database is lost after the default settings go"
step "the keys of the default database go when its last declaration goes"
scenario $h "{ $base $(user "dconf.settings = lib.mkForce { };") }"
apply $h >/dev/null || fail "apply failed"
[[ -z $(scheme) ]] || fail "a key of the default database stays after its last declaration goes"
check $h 'test "$(dconf read /org/gnome/desktop/interface/font-name)" = "'"'Mine 10'"'"' "the key that the user wrote in the default database is lost"
empty_plan $h

step "home.packages must stay empty"
scenario $h '{ features.home.enable = true; home.users.arch = { pkgs, ... }: { home.packages = [ pkgs.hello ]; }; }'
out=$(plan $h) && fail "the plan accepts home.packages"
has "$out" 'Home Manager installs these packages from Nix: hello' || { echo "$out"; fail "wrong error for home.packages"; }
pass
