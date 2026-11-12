#!/usr/bin/env bash
# Scenario 3: add and remove a repo package, a group and AUR packages, remove a whole
# feature, and restore pristine files after an override is dropped.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
repo='pacman -Slq --config /var/lib/pacnix/pacman.conf pacnix-local'

step "add a repo package and AUR packages, one with an AUR dependency"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; }'
apply $h || fail "apply failed"
check $h "pacman -Q htop downgrade perl-email-outlook-message" "declared packages are missing"
check $h "pacman -Qqd perl-ole-storage-lite" "the AUR dependency is not installed as a dependency"
check $h "$repo | grep -qx perl-ole-storage-lite" "the AUR dependency is not in the local repository"
packages_match $h

step "remove them"
scenario $h ''
apply $h || fail "apply failed"
for p in htop downgrade perl-email-outlook-message perl-ole-storage-lite; do
	refute $h "pacman -Q $p" "$p is still installed"
	refute $h "$repo | grep -qx $p" "$p is still in the local repository"
done
packages_match $h

step "a group stands for its members, as with pacman -S"
scenario $h '{ packages.repo = [ "fprint" ]; }'
apply $h >/dev/null || fail "apply failed"
check $h "pacman -Qqe fprintd libfprint" "the members of the group are not explicitly installed"
empty_plan $h
pacnix $h audit | grep -q 'explicit package not declared: .*fprint' && fail "the audit does not count the members as declared"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h "pacman -Q fprintd" "a member of the removed group is still installed"

step "a symbolic link: create, change the target, drop"
scenario $h '{ files."/etc/pacnix-zone".target = "/usr/share/zoneinfo/UTC"; }'
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/pacnix-zone)" = /usr/share/zoneinfo/UTC' "the link is not created"
scenario $h '{ files."/etc/pacnix-zone".target = "/usr/share/zoneinfo/Europe/Berlin"; }'
plan $h | grep -qx 'write file /etc/pacnix-zone (update)' || fail "the plan does not update the link"
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/pacnix-zone)" = /usr/share/zoneinfo/Europe/Berlin' "the link target is not changed"
empty_plan $h
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h 'test -L /etc/pacnix-zone' "the dropped link remains"

step "enable a feature"
scenario $h '{ features.demo.enable = true; }'
apply $h || fail "apply failed"
check $h "curl -fs localhost:8081" "the demo service does not answer"
check $h "systemctl show -p Environment sshd | grep -q DEMO=1" "the sshd drop-in is not active"
check $h "test \$(sysctl -n net.core.somaxconn) = 8192" "the sysctl trigger did not run"

step "remove the feature"
scenario $h ''
apply $h || fail "apply failed"
refute $h "pacman -Q darkhttpd" "darkhttpd is still installed"
for f in /etc/systemd/system/demo-web.service /etc/systemd/system/sshd.service.d /etc/sysctl.d/90-demo.conf; do
	refute $h "test -e $f" "$f still exists"
done
refute $h "systemctl is-enabled demo-web.service" "demo-web.service is still enabled"
refute $h "systemctl is-active demo-web.service" "demo-web.service is still running"
refute $h "find /etc/systemd/system -xtype l | grep ." "dangling unit links remain"
refute $h "systemctl show -p Environment sshd | grep -q DEMO=1" "the sshd drop-in is still active"
empty_plan $h

step "override package files"
scenario $h '{ features.overrides.enable = true; }'
apply $h || fail "apply failed"
check $h "grep -qx Color /etc/pacman.conf" "pacman.conf is not overridden"
check $h "locale -a | grep -q ar_AE" "the locale trigger did not run"
check $h "grep -q '^auth.*pam_wheel.so use_uid' /etc/pam.d/su" "the PAM file is not overridden"
empty_plan $h

step "drop the overrides and restore the pristine files"
scenario $h ''
apply $h || fail "apply failed"
for f in pacman:/etc/pacman.conf glibc:/etc/locale.gen util-linux:/etc/pam.d/su; do
	check $h "bsdtar -xOf /var/cache/pacman/pkg/${f%%:*}-\$(pacman -Q ${f%%:*} | cut -d' ' -f2)-*.pkg.tar.zst ${f#*:/} | cmp - ${f#*:}" "${f#*:} is not pristine"
done
refute $h "locale -a | grep -q ar_AE" "the locale trigger did not run after the restore"
empty_plan $h
pass
