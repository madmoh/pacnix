#!/usr/bin/env bash
# Scenario 17: switch the whole stack of a host in one apply: desktop and
# display manager (GNOME to Plasma), kernel (linux to linux-lts), initramfs
# generator (mkinitcpio to booster, a provider choice) and network service
# (systemd-networkd to NetworkManager, over the SSH connection it carries).
size=(4 6144 1)
source "$(dirname "$0")/../lib.sh"
fresh $h
step "a GNOME host"
scenario $h '{ features.desktop = "gnome"; }'
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/systemd/system/display-manager.service)" = /usr/lib/systemd/system/gdm.service' "gdm is not the display manager"
packages_match $h
step "switch the stack"
switch='{ features.desktop = "plasma"; features.base = { kernel = "linux-lts"; network = "NetworkManager"; }; packages.providers.initramfs = "booster"; }'
scenario $h "$switch"
out=$(apply $h) && fail "apply removed the booted kernel without the override"
grep -q 'protected packages: linux' <<<"$out" || { echo "$out" | tail; fail "wrong error for the booted kernel"; }
apply $h --allow-protected-removal || fail "apply of the switch failed"
for p in gnome-shell gdm linux mkinitcpio; do refute $h "pacman -Q $p" "$p is still installed"; done
check $h 'pacman -Q plasma-desktop sddm linux-lts booster networkmanager' "the new stack is not installed"
check $h 'test "$(readlink /etc/systemd/system/display-manager.service)" = /usr/lib/systemd/system/sddm.service' "sddm is not the display manager"
check $h 'systemctl is-active NetworkManager' "NetworkManager does not run"
refute $h 'systemctl is-enabled systemd-networkd.service' "systemd-networkd is still enabled"
check $h 'test -e /boot/booster-linux-lts.img' "booster made no image"
check $h 'sudo grep -q "initrd.*/boot/booster-linux-lts.img" /boot/grub/grub.cfg' "grub.cfg has no entry for the booster image"
# mkinitcpio leaves its image when it goes, and GRUB prefers that name: Arch behaviour, shown here.
echo "initrd of the default entry: $(on $h "sudo awk '/^menuentry /{n++} n==1 && \$1==\"initrd\"' /boot/grub/grub.cfg")"
packages_match $h
empty_plan $h
step "reboot into the new stack"
on $h 'sudo systemctl reboot' || true
sleep 10
"$vm" wait $h
check $h 'uname -r | grep -q lts' "the LTS kernel does not run"
check $h 'systemctl is-active NetworkManager sshd' "the network or SSH does not run after the reboot"
check $h 'test "$(systemctl is-system-running)" = running' "the system is not healthy"
empty_plan $h
pass
