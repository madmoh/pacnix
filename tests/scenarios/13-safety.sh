#!/usr/bin/env bash
# Scenario 13: a misspelled package name, a conflicting
# declaration, the removal of protected packages, a different ID of an existing
# account and two boot loaders fail before any change.
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
	grep -q "$2" <<<"$out" || { echo "$out"; fail "wrong error for $3"; }
	after=$(on $h "$state")
	[[ $before == "$after" ]] || { diff <(echo "$before") <(echo "$after"); fail "the system changed for $3"; }
	echo "refused: $3"
}
step "refuse unsafe configurations"
refused '{ packages.repo = [ "htopp" ]; }' 'unknown packages: htopp' 'a misspelled repo package'
refused '{ packages.aur = [ "downgradee" ]; }' 'unknown packages: downgradee' 'a misspelled AUR package'
refused '{ imports = [ { files."/etc/x.conf".text = "a"; } { files."/etc/x.conf".text = "b"; } ]; }' 'conflicting definition values' 'two contents for one file'
refused '{ units.mask = [ "sshd.service" ]; }' 'enabled and also masked or disabled' 'a unit that is enabled and masked'
refused '{ packages.aur = [ "htop" ]; packages.repo = [ "htop" ]; }' 'more than one of repo, aur and local' 'a package in repo and aur'
refused '{ packages.repo = [ "vim" "gvim" ]; }' 'declared packages conflict: gvim and vim' 'two declared packages that conflict'
refused '{ packages.versions.htop = "3.0.0-1"; }' 'declare these packages in repo or aur: htop' 'a version of a package that is not declared'
# The cloud image installed base and the kernel. Here pacnix installed them.
made $h base linux
refused '{ lib, ... }: { features.base.enable = lib.mkForce false; packages.repo = [ "openssh" "sudo" "cloud-init" "rsync" "git" "jq" "grub" ]; units.enable = [ "sshd.service" ]; }' 'protected packages: base' 'removal of base and the kernel'
refused '{ users.users.arch.uid = 1234; }' 'pacnix does not change the ID of an account' 'a different ID of an existing account'
refused '{ boot.loader.systemd-boot.enable = true; }' 'enable one boot loader' 'two boot loaders'
pass
