#!/usr/bin/env bash
# Scenario 7: boot.kernelParams for GRUB, and mkinitcpio settings through a
# drop-in, which the trigger initramfs builds, a reboot, and the reverse when the feature goes. The summary of
# apply and the audit show that a reboot is necessary until the reboot.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
step "declare the drop-ins"
scenario $h '{ features.boot = { enable = true; cmdline = "pacnix.test=1 mitigations=auto"; compression = "xz"; }; }'
out=$(plan $h)
has "$out" 'Run trigger loader: grub-install --target=i386-pc /dev/vda && grub-mkconfig -o /boot/grub/grub.cfg' || { echo "$out"; fail "the plan does not run grub-mkconfig"; }
changes "$out" | grep -q '^Run trigger initramfs: ' || fail "the plan does not run mkinitcpio"
out=$(apply $h) || fail "apply failed"
has "$out" '• Reboot to use the changes: /boot/initramfs-linux.img changed after the boot' || { echo "$out"; fail "apply does not tell to reboot"; }
check $h 'sudo grep -q "pacnix.test=1" /boot/grub/grub.cfg' "grub.cfg does not have the parameter"
check $h 'sudo lsinitcpio -a /boot/initramfs-linux.img 2>/dev/null | grep -q "Compressed with: xz"' "the initramfs is not xz compressed"
pacnix $h audit | grep -qx '• Reboot necessary: /boot/initramfs-linux.img changed after the boot' || fail "the audit does not show that a reboot is necessary"
step "reboot"
"$vm" reboot $h
check $h 'grep -q "pacnix.test=1" /proc/cmdline' "the booted kernel does not have the parameter"
pacnix $h audit | grep -q '^• Reboot necessary' && fail "the audit shows a reboot after the reboot"
check $h 'test "$(systemctl is-system-running)" = running' "the system is not healthy after the reboot"
empty_plan $h
step "remove the drop-ins"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h 'sudo grep -q "pacnix.test=1" /boot/grub/grub.cfg' "grub.cfg still has the parameter"
refute $h 'sudo lsinitcpio -a /boot/initramfs-linux.img 2>/dev/null | grep -q "Compressed with: xz"' "the initramfs is still xz compressed"
"$vm" reboot $h
refute $h 'grep -q "pacnix.test=1" /proc/cmdline' "the booted kernel still has the parameter"
empty_plan $h
pass
