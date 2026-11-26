#!/usr/bin/env bash
# Scenario 18: a pinned version. A repo package at an older version comes from
# the Arch Linux Archive, pacnix apply keeps it and corrects a manual
# upgrade, and an AUR package at a version that the AUR does not have is refused.
# pacnix apply upgrades an AUR package without a question and without a diff
# of its files. A new maintainer of a built AUR package is a notice one time.
source "$(dirname "$0")/../lib.sh"
fresh $h
cache=$(cache_list $h)
now=$(on $h "pacman -Si tree | awk '/^Version/ { print \$3 }'")
listing=$(curl -fsS --connect-timeout 10 --max-time 30 https://archive.archlinux.org/packages/t/tree/) || fail "blocked: the host cannot read https://archive.archlinux.org/packages/t/tree/ (curl exit status $?)"
old=$(grep -o 'tree-[0-9][^"]*-x86_64\.pkg\.tar\.zst"' <<<"$listing" | sed 's/^tree-//; s/-x86_64.*//' | grep -vx "$now" | sort -V | tail -1)
[[ -n $old ]] || fail "the archive has no older version of tree"
step "pin tree to $old (the repository has $now)"
scenario $h "{ packages.repo = [ \"tree\" ]; packages.versions.tree = \"$old\"; }"
out=$(plan $h)
has "$out" 'Install package from the Arch Linux Archive https://archive.archlinux.org/packages/t/tree/tree-' || { echo "$out"; fail "the plan does not use the archive"; }
apply $h >/dev/null || fail "apply failed"
check $h "test \"\$(pacman -Q tree)\" = 'tree $old'" "tree is not at the pinned version"
empty_plan $h
step "pacnix apply keeps the pinned version"
pacnix $h apply >/dev/null || fail "upgrade failed"
check $h "test \"\$(pacman -Q tree)\" = 'tree $old'" "the upgrade changed the pinned version"
step "apply corrects a manual upgrade"
cache_kept $h "$cache"
on $h 'sudo pacman -S --noconfirm tree >/dev/null 2>&1'
# The manual upgrade with pacman downloads into /var/cache/pacman/pkg.
cache=$(cache_list $h)
check $h "test \"\$(pacman -Q tree)\" = 'tree $now'" "the manual upgrade did not happen"
apply $h >/dev/null || fail "apply failed"
check $h "test \"\$(pacman -Q tree)\" = 'tree $old'" "apply did not restore the pinned version"
empty_plan $h
step "an AUR version that the AUR does not have"
scenario $h '{ packages.aur = [ "downgrade" ]; packages.versions.downgrade = "0.1-1"; }'
out=$(apply $h 2>&1) && fail "apply accepted an AUR version that does not exist"
has "$out" 'downgrade 0.1-1 (the AUR or the local repository has' || { echo "$out" | tail -3; fail "the refusal does not name the version"; }
step "pacnix apply shows the changes of an AUR package before the build"
# An older revision of the AUR package downgrade, built as a local package.
on $h 'sudo git clone -q https://aur.archlinux.org/downgrade.git /etc/pacnix/old-downgrade && cd /etc/pacnix/old-downgrade && v=$(sudo git show HEAD:.SRCINFO | grep -m1 pkgver) && for c in $(sudo git rev-list HEAD -- .SRCINFO); do sudo git show $c:.SRCINFO | grep -qx "$v" || { sudo git checkout -q $c; break; }; done && sudo rm -rf .git' || fail "no older revision of downgrade"
scenario $h '{ packages.local.downgrade = ./old-downgrade; }'
apply $h >/dev/null || fail "apply of the older downgrade failed"
old=$(on $h 'pacman -Q downgrade')
# The news of the time before the test are not part of it.
on $h 'date +%s | sudo tee /var/lib/pacnix/news >/dev/null'
scenario $h '{ packages.aur = [ "downgrade" ]; }'
out=$(pacnix $h apply 2>&1) || { echo "$out" | tail -5; fail "the upgrade of the AUR package failed or asked"; }
grep -q '^+pkgname=downgrade' <<<"$out" && { echo "$out" | tail -5; fail "the upgrade shows a diff of the package"; }
check $h "test \"\$(pacman -Q downgrade)\" != '$old'" "the AUR package is not upgraded"
step "a new maintainer of a built AUR package is a notice one time"
on $h "sudo jq '.downgrade.maintainer = \"someone-else\"' /var/lib/pacnix/aur.json | sudo tee /var/lib/pacnix/aur.json.new >/dev/null && sudo mv /var/lib/pacnix/aur.json.new /var/lib/pacnix/aur.json && sudo jq 'del(.downgrade)' /var/lib/pacnix/pruned.json | sudo tee /var/lib/pacnix/pruned.json.new >/dev/null && sudo mv /var/lib/pacnix/pruned.json.new /var/lib/pacnix/pruned.json"
out=$(apply $h) || { echo "$out" | tail -5; fail "apply failed"; }
matches "$out" '• The AUR package downgrade has new maintainers: .*, before: someone-else \(co-maintainers: .*\)\.' || { echo "$out" | tail -5; fail "apply does not show the new maintainer"; }
out=$(apply $h) || { echo "$out" | tail -5; fail "the next apply failed"; }
grep -q 'new maintainers' <<<"$out" && { echo "$out" | tail -5; fail "the next apply shows the notice again"; }
cache_kept $h "$cache"
pass
