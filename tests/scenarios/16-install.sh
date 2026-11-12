#!/usr/bin/env bash
# Scenario 16: install to an empty disk. A live system (the role of the Arch
# ISO) partitions, formats and mounts the disk, pacstraps base, nix and the
# pacnix package, and runs pacnix apply in arch-chroot. The disk then boots
# alone, without cloud-init, and pacnix-firstboot.service builds the AUR and
# local packages and activates the home configuration.
size=(6 6144 1)
source "$(dirname "$0")/../lib.sh"
fresh live "$cache/base-20261101.620452.qcow2"
"$vm" blank live 12G
step "install from the live system"
on live "sudo tee /etc/pacnix/scenario.nix >/dev/null" <<<"{ features.machine.sshKey = \"$(cat "$cache/id_ed25519.pub")\n\"; }"
on live 'set -e
printf "label: dos\n,,L,*\n" | sudo sfdisk -q /dev/vdb
sudo mkfs.ext4 -qL root /dev/vdb1
sudo mount /dev/vdb1 /mnt
sudo pacstrap -K /mnt base nix >/dev/null
sudo pacstrap -U /mnt ~/pacnix-*.pkg.tar.zst >/dev/null
sudo cp -a /etc/pacnix /mnt/etc/pacnix' || fail "pacstrap failed"
out=$(on live 'sudo arch-chroot /mnt pacnix apply --host newbox --yes 2>&1') || { echo "$out" | tail -30; fail "apply in arch-chroot failed"; }
grep '^note:' <<<"$out"
grep -q '^note: the first boot builds and installs: aurutils pacnix$' <<<"$out" || fail "the plan does not defer the builds"
grep -q '^pacnix: note: /etc/resolv.conf is a mount point in this chroot' <<<"$out" || fail "apply does not leave /etc/resolv.conf to the running system"
grep -q '^pacnix: note: pacnix-firstboot.service completes' <<<"$out" || fail "apply does not prepare the first boot"
# pacstrap -K starts a gpg-agent for the new keyring, which keeps /mnt busy.
on live 'sudo gpgconf --homedir /mnt/etc/pacman.d/gnupg --kill gpg-agent && sudo umount -R /mnt' || fail "umount failed"
on live 'sudo systemctl poweroff' || true
while virsh -c qemu:///system domstate pacnix-live 2>/dev/null | grep -q running; do sleep 2; done
mv "$cache/pacnix-live-blank.qcow2" "$cache/newbox-disk.qcow2"
"$vm" down live
step "boot the installed disk"
started+=(newbox)
"$vm" boot newbox "$cache/newbox-disk.qcow2"
check newbox 'test "$(id -nG)" = "arch wheel" && test "$(getent passwd arch | cut -d: -f7)" = /usr/bin/zsh' "the user, its groups or its shell are wrong"
check newbox 'test "$(readlink /etc/localtime)" = /usr/share/zoneinfo/Europe/Berlin' "the time zone link is wrong"
check newbox 'locale -a | grep -qx en_US.utf8' "the locale is not generated"
check newbox 'systemctl is-active sshd systemd-networkd' "declared units do not run"
check newbox 'test "$(systemctl is-system-running)" = running' "the system is not healthy"
step "the first boot completes the configuration"
on newbox 'for i in $(seq 900); do test -e /var/lib/pacnix/firstboot || systemctl is-active -q pacnix-firstboot || exit 0; systemctl is-failed -q pacnix-firstboot && exit 1; sleep 2; done; exit 1' || {
  on newbox 'journalctl -u pacnix-firstboot --no-pager | tail -30'; fail "the first boot did not complete the configuration"; }
check newbox 'pacman -Q aurutils pacnix' "the AUR and local packages are not installed"
check newbox 'test "$(readlink /etc/resolv.conf)" = /run/systemd/resolve/stub-resolv.conf' "the resolv.conf link is not written"
check newbox 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the home configuration is not active"
packages_match newbox
empty_plan newbox
pass
