#!/usr/bin/env bash
# Scenario 16: install to an empty disk. A live system (the role of the Arch
# ISO) partitions, formats and mounts the disk, pacstraps base and pacnix from
# [pacnix], which brings aurutils built, and runs pacnix apply in arch-chroot,
# which lists the deferred work and the next command. The configuration is a
# Git repository in the home of the user, and /etc/pacnix links to it. The
# disk then boots alone, without cloud-init, and the first pacnix upgrade, as
# the user without sudo and without --yes, builds the local package and
# activates the home configuration: a new installation asks nothing. Nix
# evaluates as the user, no file in the home belongs to root, and the builder
# user has no sudo rule. The local package slow first
# fails with a message that has "timed out": pacnix stops one time with the
# reason and does not try again. After the correction, the build is silent for
# 65 s: the output shows each step, a line for the silence and the end, without
# Nix notes, progress meter redraws or escape sequences.
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
sudo pacstrap -K /mnt base pacnix >/dev/null
sudo cp /etc/pacman.conf /mnt/etc/pacman.conf
sudo cp -a /srv/pacnix /mnt/srv/
sudo mkdir -p /mnt/home/arch
sudo cp -a /etc/pacnix /mnt/home/arch/config
echo fail | sudo tee /mnt/home/arch/config/packages/slow/mode >/dev/null
sudo git -C /mnt/home/arch/config init -q
sudo git -C /mnt/home/arch/config add -A
sudo chown -R 1000:1000 /mnt/home/arch
sudo ln -s /home/arch/config /mnt/etc/pacnix' || fail "pacstrap failed"
out=$(on live 'sudo arch-chroot /mnt pacnix apply --host newbox --yes 2>&1') || { echo "$out" | tail -30; fail "apply in arch-chroot failed"; }
grep '^note:\|^pacnix: \(these\|next\)' <<<"$out"
for l in 'note: the first pacnix upgrade on the booted system builds and installs: slow' \
	'pacnix: these packages wait for the booted system: slow' \
	'pacnix: these home configurations wait for the booted system: arch' \
	'pacnix: next step: reboot, log in, connect to the network, then run: pacnix upgrade'; do
	grep -qxF "$l" <<<"$out" || fail "the apply in the chroot does not show: $l"
done
[[ $(tail -1 <<<"$out") == 'pacnix: next step: reboot, log in, connect to the network, then run: pacnix upgrade' ]] || fail "the next command is not the last line"
grep -qxF 'pacnix: /etc/resolv.conf is a mount point in this chroot. Write it after you leave the chroot, for example: ln -sf /run/systemd/resolve/stub-resolv.conf /mnt/etc/resolv.conf' <<<"$out" || fail "apply does not tell how to write /etc/resolv.conf"
on live 'sudo ln -sf /run/systemd/resolve/stub-resolv.conf /mnt/etc/resolv.conf' || fail "the link of /etc/resolv.conf cannot be written"
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
check newbox 'pacman -Qi aurutils | grep -qx "Install Reason  : Installed as a dependency for another package"' "aurutils is not a dependency of pacnix"
refute newbox 'sudo -l -U pacnix | grep -q "may run"' "the builder user has a sudo rule"
check newbox 'test "$(systemctl is-system-running)" = running' "the system is not healthy"
step "a build failure with \"timed out\" in its output stops pacnix one time with the reason"
out=$(pacnix newbox upgrade 2>&1) && { echo "$out" | tail -40; fail "the upgrade with the failing build succeeded"; }
echo "$out" | tail -8
grep -q '^pacnix: error: build local package 1/1: slow: aur chroot .* failed with exit status: 255' <<<"$out" || fail "the error does not name the build and its status"
grep -q '^ *error: the test build timed out while the file mode has the word fail' <<<"$out" || fail "the error does not show the reason of the failure"
grep -q 'attempt 1 of' <<<"$out" && fail "pacnix tried a build failure again"
grep -q 'aurutils' <<<"$out" && fail "the upgrade builds aurutils"
step "the first pacnix upgrade completes the configuration without a question"
on newbox 'echo ok >/etc/pacnix/packages/slow/mode'
out=$(pacnix newbox upgrade 2>&1) || { echo "$out" | tail -30; fail "the first upgrade failed"; }
for l in 'pacnix: build local package 1/1: slow' 'pacnix: the step continues after 1 min without output: build local package 1/1: slow' '==> colored line' 'progress done'; do
	grep -qxF "$l" <<<"$out" || fail "the output does not show: $l"
done
grep -q 'progress 10%\|not writing modified lock file\|Added input' <<<"$out" && fail "the output shows progress meter redraws or Nix notes"
grep -q $'\e\|\r' <<<"$out" && fail "the output has escape sequences or carriage returns"
check newbox 'pacman -Q slow' "the local package is not installed"
check newbox 'test "$(stat -c %U ~/.cache/nix)" = arch' "Nix does not evaluate as the owner of the flake"
refute newbox 'sudo find /home/arch -user root | grep -q .' "a file in the home belongs to root"
refute newbox 'sudo find /var/lib/pacnix/repo ! -user root | grep -q .' "the builder user owns a file of the local repository"
check newbox 'sudo gpg --homedir /var/lib/pacnix/gnupg --verify /var/lib/pacnix/repo/pacnix-local.db.sig /var/lib/pacnix/repo/pacnix-local.db' "the local repository is not signed"
check newbox 'test "$(readlink /etc/resolv.conf)" = /run/systemd/resolve/stub-resolv.conf' "the resolv.conf link is not written"
check newbox 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the home configuration is not active"
packages_match newbox
empty_plan newbox
step "a flake in a dirty Git tree gives no Nix notes"
out=$(plan newbox) || fail "plan failed in a Git tree"
grep -q 'warning:\|Added input' <<<"$out" && { echo "$out"; fail "the plan shows Nix notes"; }
pass
