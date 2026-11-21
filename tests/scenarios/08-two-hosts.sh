#!/usr/bin/env bash
# Scenario 8: one config repository with shared and host-specific features.
# desktop builds the AUR repository, signs it and serves it, laptop installs
# from it after a check of the declared key, then newpc joins as a new
# machine. A wrong key and a changed package stop newpc before any change. The
# apply of desktop removes the package files and signatures that the signed
# database does not name, keeps the served packages and the other files, and
# does not change the database.
size=(4 4096 3)
source "$(dirname "$0")/../lib.sh"
plain=$cache/base-20261101.620452.qcow2
fresh_all "$plain" desktop laptop newpc
step "desktop builds and serves the repository"
out=$(apply desktop) || { echo "$out" | tail -30; fail "apply on desktop failed"; }
packages_match desktop
key=$(on desktop "sudo gpg --homedir /var/lib/pacnix/gnupg --with-colons --list-keys | awk -F: '/^fpr/ { print \$10; exit }'")
has "$out" "• New key of the local repository, for packages.serverKey of other hosts: $key" || fail "apply does not show the key of the new repository"
served='curl -fsS http://localhost:8080/pacnix-local.db | bsdtar -xOf - | grep -A1 "^%NAME%" | grep -v "^%NAME%\|^--"'
for p in downgrade perl-email-outlook-message perl-ole-storage-lite; do
	check desktop "$served | grep -qx $p" "the repository of desktop does not serve $p"
done
refute desktop 'pacman -Q downgrade' "desktop installs the AUR package of laptop"
on desktop 'cd /var/lib/pacnix/repo && f=$(ls downgrade-[0-9]*.pkg.tar.zst) && sudo cp $f downgrade-0.1-1-any.pkg.tar.zst && sudo cp $f.sig downgrade-0.1-1-any.pkg.tar.zst.sig && sudo touch other.txt'
db=$(on desktop 'md5sum /var/lib/pacnix/repo/pacnix-local.db.tar.gz*')
apply desktop >/dev/null || fail "apply on desktop failed"
refute desktop 'ls /var/lib/pacnix/repo/downgrade-0.1-1-any.pkg.tar.zst*' "an old package file stays in the repository"
check desktop 'cd /var/lib/pacnix/repo && ls downgrade-[0-9]*.pkg.tar.zst.sig perl-ole-storage-lite-*.pkg.tar.zst.sig other.txt' "the apply removed a served package or a file that is not a package"
[[ $db == $(on desktop 'md5sum /var/lib/pacnix/repo/pacnix-local.db.tar.gz*') ]] || fail "the apply changed the database of the repository"
check desktop 'grep -qx HandleLidSwitch=ignore /etc/systemd/logind.conf.d/50-lid.conf' "the host value of desktop is wrong"
step "laptop installs from the repository of desktop"
scenario laptop "{ packages.serverKey = \"$key\"; }"
apply laptop || fail "apply on laptop failed"
packages_match laptop
check laptop 'pacman -Q downgrade zsh-antidote' "laptop lacks AUR packages"
refute laptop 'ls /var/lib/pacnix/repo/*.pkg.tar.zst' "laptop built packages"
check laptop 'grep -qx HandleLidSwitch=suspend /etc/systemd/logind.conf.d/50-lid.conf' "the host value of laptop is wrong"
check laptop 'swapon --show=NAME,SIZE --noheadings | grep -q "/swapfile *256M"' "the swap file of laptop has the wrong size"
check laptop 'systemctl is-active tailscaled avahi-daemon' "shared units do not run on laptop"
empty_plan laptop
empty_plan desktop
"$vm" down laptop
step "newpc joins as a new machine"
scenario newpc '{ packages.serverKey = "0000000000000000000000000000000000000000"; }'
out=$(apply newpc) && fail "apply on newpc with a wrong key succeeded"
has "$out" "• Error: the key in http://desktop:8080/pacnix-local.asc has the fingerprint $key, but packages.serverKey is 0000" || { echo "$out" | tail; fail "the error does not show the wrong key"; }
refute newpc 'pacman -Q zsh-antidote' "newpc installed a package with a wrong key"
scenario newpc "{ packages.serverKey = \"$key\"; }"
on desktop 'f=$(ls /var/lib/pacnix/repo/trash-d-[0-9]*.pkg.tar.zst) && sudo cp "$f" /root/ && printf x | sudo dd of="$f" bs=1 seek=100 conv=notrunc status=none'
out=$(apply newpc) && fail "apply on newpc with a changed package succeeded"
has "$out" 'error: failed to commit transaction (invalid or corrupted package)' || { echo "$out" | tail; fail "pacman does not refuse the changed package"; }
refute newpc 'pacman -Q zsh-antidote' "newpc installed packages with a changed package"
on desktop "sudo sh -c 'cp /root/trash-d-[0-9]*.pkg.tar.zst /var/lib/pacnix/repo/'"
apply newpc || fail "apply on newpc failed"
packages_match newpc
check newpc 'pacman -Q aurutils pacnix zsh-antidote trash-d' "newpc lacks AUR packages"
refute newpc 'pacman -Q downgrade' "newpc has the host-specific package of laptop"
empty_plan newpc
pass
