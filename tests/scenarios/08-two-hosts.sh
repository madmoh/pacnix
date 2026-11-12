#!/usr/bin/env bash
# Scenario 8: one config repository with shared and host-specific features.
# desktop builds the AUR repository and serves it, laptop installs from it,
# then newpc joins as a new machine.
size=(4 6144 2)
source "$(dirname "$0")/../lib.sh"
plain=$cache/base-20261101.620452.qcow2
fresh desktop "$plain"
fresh laptop "$plain"
step "desktop builds and serves the repository"
apply desktop || fail "apply on desktop failed"
packages_match desktop
served='curl -fsS http://localhost:8080/pacnix-local.db | bsdtar -xOf - | grep -A1 "^%NAME%" | grep -v "^%NAME%\|^--"'
for p in downgrade perl-email-outlook-message perl-ole-storage-lite aurutils pacnix; do
	check desktop "$served | grep -qx $p" "the repository of desktop does not serve $p"
done
refute desktop 'pacman -Q downgrade' "desktop installs the AUR package of laptop"
check desktop 'grep -qx HandleLidSwitch=ignore /etc/systemd/logind.conf.d/50-lid.conf' "the host value of desktop is wrong"
step "laptop installs from the repository of desktop"
apply laptop || fail "apply on laptop failed"
packages_match laptop
check laptop 'pacman -Q downgrade zsh-antidote' "laptop lacks AUR packages"
refute laptop 'test -e /var/lib/aurbuild/x86_64/root' "laptop built packages"
check laptop 'grep -qx HandleLidSwitch=suspend /etc/systemd/logind.conf.d/50-lid.conf' "the host value of laptop is wrong"
check laptop 'swapon --show=NAME,SIZE --noheadings | grep -q "/swapfile *256M"' "the swap file of laptop has the wrong size"
check laptop 'systemctl is-active tailscaled avahi-daemon' "shared units do not run on laptop"
empty_plan laptop
empty_plan desktop
"$vm" down laptop
step "newpc joins as a new machine"
fresh newpc "$plain"
apply newpc || fail "apply on newpc failed"
packages_match newpc
check newpc 'pacman -Q aurutils pacnix zsh-antidote trash-d' "newpc lacks AUR packages"
refute newpc 'pacman -Q downgrade' "newpc has the host-specific package of laptop"
empty_plan newpc
pass
