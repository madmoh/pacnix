#!/usr/bin/env bash
# Scenario 13: a misspelled package name, a conflicting declaration, a
# different ID of an existing account and two boot loaders fail before any
# change. pacnix stays when its declaration goes. With --no-config, an error in
# the configuration does not stop the upgrade.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
state='pacman -Q | md5sum; readlink /nix/var/nix/profiles/pacnix; ls /var/lib/pacnix/pending 2>/dev/null; find /etc /var/lib/pacman/local -newer /var/tmp/pacnix-marker -not -path "/etc/pacnix*" 2>/dev/null | sort'
# Applies a scenario that must fail with a message, and checks that nothing changed.
refused() {
	local before after out
	scenario $h "$1"
	on $h "sudo touch /var/tmp/pacnix-marker"
	before=$(on $h "$state")
	out=$(apply $h) && fail "apply accepted: $3"
	has "$out" "$2" || { echo "$out"; fail "wrong error for $3"; }
	after=$(on $h "$state")
	[[ $before == "$after" ]] || { diff <(echo "$before") <(echo "$after"); fail "the system changed for $3"; }
	echo "refused: $3"
}
step "refuse unsafe configurations"
refused '{ packages.repo = [ "htopp" ]; }' 'packages.repo: unknown package htopp' 'a misspelled repo package'
refused '{ packages.aur = [ "downgradee" ]; }' 'packages.aur: unknown package downgradee' 'a misspelled AUR package'
refused '{ imports = [ { files."/etc/x.conf".text = "a"; } { files."/etc/x.conf".text = "b"; } ]; }' 'conflicting definition values' 'two contents for one file'
refused '{ units.mask = [ "sshd.service" ]; }' 'enabled and also masked or disabled' 'a unit that is enabled and masked'
refused '{ packages.aur = [ "htop" ]; packages.repo = [ "htop" ]; }' 'more than one of repo, aur and local' 'a package in repo and aur'
refused '{ packages.repo = [ "vim" "gvim" ]; }' 'declared packages conflict: gvim and vim' 'two declared packages that conflict'
refused '{ packages.versions.htop = "3.0.0-1"; }' 'declare these packages in repo or aur: htop' 'a version of a package that is not declared'
refused '{ users.users.arch.uid = 1234; }' 'pacnix does not change the ID of an account' 'a different ID of an existing account'
refused '{ boot.loader.systemd-boot.enable = true; }' 'enable one boot loader' 'two boot loaders'
step "with --no-config, an error in the configuration does not stop the upgrade"
out=$(apply $h --no-config) || { echo "$out" | tail; fail "apply --no-config failed at an error in the configuration"; }
has "$out" '• The plan failed: ' || { echo "$out" | tail; fail "apply --no-config does not show the error of the plan"; }
has "$out" 'System upgrade' || { echo "$out" | tail; fail "apply --no-config did not upgrade"; }
step "pacnix stays when its declaration goes"
scenario $h '{ packages.repo = [ "pacnix" ]; }'
apply $h >/dev/null || fail "apply that declares pacnix failed"
made $h pacnix
scenario $h ''
apply $h >/dev/null || fail "apply without the declaration of pacnix failed"
check $h 'pacman -Qqe pacnix' "pacnix is removed or no longer explicitly installed"
pass
