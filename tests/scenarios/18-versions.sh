#!/usr/bin/env bash
# Scenario 18: a pinned version. A repo package at an older version comes from
# the Arch Linux Archive, pacnix upgrade keeps it, apply corrects a manual
# upgrade, and an AUR package at a version that the AUR does not have is refused.
# pacnix upgrade shows the changes of an AUR package and asks before the build.
source "$(dirname "$0")/../lib.sh"
fresh $h
now=$(on $h "pacman -Si tree | awk '/^Version/ { print \$3 }'")
old=$(curl -fsS https://archive.archlinux.org/packages/t/tree/ | grep -o 'tree-[0-9][^"]*-x86_64\.pkg\.tar\.zst"' | sed 's/^tree-//; s/-x86_64.*//' | grep -vx "$now" | sort -V | tail -1)
[[ -n $old ]] || fail "the archive has no older version of tree"
step "pin tree to $old (the repository has $now)"
scenario $h "{ packages.repo = [ \"tree\" ]; packages.versions.tree = \"$old\"; }"
plan $h | grep -q '^install package from the Arch Linux Archive https://archive.archlinux.org/packages/t/tree/' || fail "the plan does not use the archive"
apply $h >/dev/null || fail "apply failed"
check $h "test \"\$(pacman -Q tree)\" = 'tree $old'" "tree is not at the pinned version"
empty_plan $h
step "pacnix upgrade keeps the pinned version"
pacnix $h upgrade --yes >/dev/null || fail "upgrade failed"
check $h "test \"\$(pacman -Q tree)\" = 'tree $old'" "the upgrade changed the pinned version"
step "apply corrects a manual upgrade"
on $h 'sudo pacman -S --noconfirm tree >/dev/null 2>&1'
check $h "test \"\$(pacman -Q tree)\" = 'tree $now'" "the manual upgrade did not happen"
apply $h >/dev/null || fail "apply failed"
check $h "test \"\$(pacman -Q tree)\" = 'tree $old'" "apply did not restore the pinned version"
empty_plan $h
step "an AUR version that the AUR does not have"
scenario $h '{ packages.aur = [ "downgrade" ]; packages.versions.downgrade = "0.1-1"; }'
out=$(apply $h 2>&1) && fail "apply accepted an AUR version that does not exist"
grep -q 'downgrade 0.1-1 (the AUR or the local repository has' <<<"$out" || { echo "$out" | tail -3; fail "the refusal does not name the version"; }
step "pacnix upgrade shows the changes of an AUR package before the build"
# An older revision of the AUR package downgrade, built as a local package.
on $h 'sudo git clone -q https://aur.archlinux.org/downgrade.git /etc/pacnix/old-downgrade && cd /etc/pacnix/old-downgrade && v=$(sudo git show HEAD:.SRCINFO | grep -m1 pkgver) && for c in $(sudo git rev-list HEAD -- .SRCINFO); do sudo git show $c:.SRCINFO | grep -qx "$v" || { sudo git checkout -q $c; break; }; done && sudo rm -rf .git' || fail "no older revision of downgrade"
scenario $h '{ packages.local.downgrade = ./old-downgrade; }'
apply $h >/dev/null || fail "apply of the older downgrade failed"
old=$(on $h 'pacman -Q downgrade')
scenario $h '{ packages.aur = [ "downgrade" ]; }'
apply $h >/dev/null || fail "apply of the AUR package failed"
out=$(pacnix $h upgrade 2>&1) && fail "upgrade built the changes without a confirmation"
grep -q '^+pkgname=downgrade' <<<"$out" || { echo "$out" | tail -5; fail "the upgrade does not show the files of the package"; }
grep -q 'Build 1 AUR packages with these changes?' <<<"$out" || { echo "$out" | tail -3; fail "the upgrade does not ask for a confirmation"; }
pacnix $h upgrade --yes >/dev/null || fail "upgrade --yes failed"
check $h "test \"\$(pacman -Q downgrade)\" != '$old'" "the AUR package is not upgraded"
pacnix $h upgrade >/dev/null || fail "the upgrade asks again for reviewed changes"
pass
