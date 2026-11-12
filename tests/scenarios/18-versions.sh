#!/usr/bin/env bash
# Scenario 18: a pinned version. A repo package at an older version comes from
# the Arch Linux Archive, pacnix upgrade keeps it, apply corrects a manual
# upgrade, and an AUR package at a version that the AUR does not have is refused.
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
pass
