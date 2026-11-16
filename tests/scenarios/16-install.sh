#!/usr/bin/env bash
# Scenario 16: install to an empty disk. A live system (the role of the Arch
# ISO) partitions, formats and mounts the disk, pacstraps base, nix and the
# pacnix package, and runs pacnix apply in arch-chroot, which lists the
# deferred work and the next command. The disk then boots alone, without
# cloud-init, and the first pacnix upgrade, with an empty local repository and
# without --yes, builds the AUR and local packages and activates the home
# configuration: a new installation asks nothing. The local package slow first
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
sudo pacstrap -K /mnt base nix >/dev/null
sudo pacstrap -U /mnt ~/pacnix-*.pkg.tar.zst >/dev/null
sudo cp -a /etc/pacnix /mnt/etc/pacnix
echo fail | sudo tee /mnt/etc/pacnix/packages/slow/mode >/dev/null' || fail "pacstrap failed"
out=$(on live 'sudo arch-chroot /mnt pacnix apply --host newbox --yes 2>&1') || { echo "$out" | tail -30; fail "apply in arch-chroot failed"; }
grep '^note:\|^pacnix: \(these\|next\)' <<<"$out"
for l in 'note: the first pacnix upgrade on the booted system builds and installs: aurutils pacnix slow' \
	'pacnix: these packages wait for the booted system: aurutils pacnix slow' \
	'pacnix: these home configurations wait for the booted system: arch' \
	'pacnix: next step: reboot, log in, connect to the network, then run: sudo pacnix upgrade'; do
	grep -qxF "$l" <<<"$out" || fail "the apply in the chroot does not show: $l"
done
[[ $(tail -1 <<<"$out") == 'pacnix: next step: reboot, log in, connect to the network, then run: sudo pacnix upgrade' ]] || fail "the next command is not the last line"
grep -q '^pacnix: note: /etc/resolv.conf is a mount point in this chroot' <<<"$out" || fail "apply does not leave /etc/resolv.conf to the running system"
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
step "a build failure with \"timed out\" in its output stops pacnix one time with the reason"
out=$(pacnix newbox upgrade 2>&1) && { echo "$out" | tail -40; fail "the upgrade with the failing build succeeded"; }
echo "$out" | tail -8
grep -q '^pacnix: error: build local package 2/2: slow: aur build .* failed with exit status: 255' <<<"$out" || fail "the error does not name the build and its status"
grep -q '^ *error: the test build timed out while the file mode has the word fail' <<<"$out" || fail "the error does not show the reason of the failure"
grep -q 'attempt 1 of' <<<"$out" && fail "pacnix tried a build failure again"
grep -qxF 'pacnix: build aurutils from the AUR, because the plan needs it' <<<"$out" || fail "the upgrade with an empty local repository does not build aurutils"
step "the first pacnix upgrade completes the configuration without a question"
on newbox 'echo ok | sudo tee /etc/pacnix/packages/slow/mode >/dev/null'
out=$(pacnix newbox upgrade 2>&1) || { echo "$out" | tail -30; fail "the first upgrade failed"; }
for l in 'pacnix: build local package 1/2: pacnix' 'pacnix: the step continues after 1 min without output: build local package 2/2: slow' '==> colored line' 'progress done'; do
	grep -qxF "$l" <<<"$out" || fail "the output does not show: $l"
done
grep -q 'progress 10%\|not writing modified lock file\|Added input' <<<"$out" && fail "the output shows progress meter redraws or Nix notes"
grep -q $'\e\|\r' <<<"$out" && fail "the output has escape sequences or carriage returns"
check newbox 'sudo jq -e ".aurutils.maintainer != \"\"" /var/lib/pacnix/aur.json' "the first build does not record the maintainer of aurutils"
check newbox 'pacman -Q aurutils pacnix' "the AUR and local packages are not installed"
check newbox 'test "$(readlink /etc/resolv.conf)" = /run/systemd/resolve/stub-resolv.conf' "the resolv.conf link is not written"
check newbox 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the home configuration is not active"
packages_match newbox
empty_plan newbox
step "a flake in a dirty Git tree gives no Nix notes"
on newbox 'sudo git -C /etc/pacnix init -q && sudo git -C /etc/pacnix add -A'
out=$(plan newbox) || fail "plan failed in a Git tree"
grep -q 'warning:\|Added input' <<<"$out" && { echo "$out"; fail "the plan shows Nix notes"; }
pass
