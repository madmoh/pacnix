#!/usr/bin/env bash
# Scenario 7: kernel command line and mkinitcpio settings through drop-ins and
# triggers, a reboot, and the reverse when the feature goes.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
step "declare the drop-ins"
scenario $h '{ features.boot = { enable = true; cmdline = "pacnix.test=1 mitigations=auto"; compression = "xz"; }; }'
out=$(plan $h)
grep -qx 'run trigger grub: grub-mkconfig -o /boot/grub/grub.cfg' <<<"$out" || { echo "$out"; fail "the plan does not run grub-mkconfig"; }
grep -qx 'run trigger mkinitcpio: mkinitcpio -P' <<<"$out" || fail "the plan does not run mkinitcpio"
apply $h >/dev/null || fail "apply failed"
check $h 'sudo grep -q "pacnix.test=1" /boot/grub/grub.cfg' "grub.cfg does not have the parameter"
check $h 'sudo lsinitcpio -a /boot/initramfs-linux.img 2>/dev/null | grep -q "Compressed with: xz"' "the initramfs is not xz compressed"
step "reboot"
on $h 'sudo systemctl reboot' || true
sleep 10
"$vm" wait $h
check $h 'grep -q "pacnix.test=1" /proc/cmdline' "the booted kernel does not have the parameter"
check $h 'test "$(systemctl is-system-running)" = running' "the system is not healthy after the reboot"
empty_plan $h
step "remove the drop-ins"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h 'sudo grep -q "pacnix.test=1" /boot/grub/grub.cfg' "grub.cfg still has the parameter"
refute $h 'sudo lsinitcpio -a /boot/initramfs-linux.img 2>/dev/null | grep -q "Compressed with: xz"' "the initramfs is still xz compressed"
on $h 'sudo systemctl reboot' || true
sleep 10
"$vm" wait $h
refute $h 'grep -q "pacnix.test=1" /proc/cmdline' "the booted kernel still has the parameter"
empty_plan $h
pass
