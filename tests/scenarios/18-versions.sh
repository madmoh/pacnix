#!/usr/bin/env bash
# Scenario 18: a pinned version. A repo package at an older version comes from
# the Arch Linux Archive, pacnix upgrade keeps it, apply corrects a manual
# upgrade, and an AUR package at a version that the AUR does not have is refused.
# pacnix upgrade shows the changes of an AUR package before the build, without
# a question. A new maintainer of a built AUR package needs a confirmation.
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
# The news of the time before the test are not part of it.
on $h 'date +%s | sudo tee /var/lib/pacnix/news >/dev/null'
out=$(pacnix $h upgrade 2>&1) || { echo "$out" | tail -5; fail "the upgrade of the AUR package failed or asked"; }
grep -qx 'pacnix: changes of the AUR package downgrade since its last build:' <<<"$out" || fail "the upgrade does not show the changes of the package"
grep -q '^+pkgname=downgrade' <<<"$out" || { echo "$out" | tail -5; fail "the upgrade does not show the files of the package"; }
check $h "test \"\$(pacman -Q downgrade)\" != '$old'" "the AUR package is not upgraded"
step "a new maintainer of a built AUR package needs a confirmation"
on $h "sudo jq '.downgrade.maintainer = \"someone-else\"' /var/lib/pacnix/aur.json | sudo tee /var/lib/pacnix/aur.json.new >/dev/null && sudo mv /var/lib/pacnix/aur.json.new /var/lib/pacnix/aur.json && sudo -u pacnix repo-remove -q /var/lib/pacnix/repo/pacnix-local.db.tar.gz downgrade"
out=$(pacnix $h apply 2>&1) && fail "apply built a package with a new maintainer without a confirmation"
grep -q '^The AUR package downgrade has new maintainers: .*, before: someone-else (co-maintainers: .*).$' <<<"$out" || { echo "$out" | tail -5; fail "apply does not show the new maintainer"; }
grep -qx 'pacnix: error: these items need a confirmation. Give --yes to continue without a terminal' <<<"$out" || fail "apply without a terminal does not ask for --yes"
apply $h >/dev/null || fail "apply --yes failed"
out=$(pacnix $h apply 2>&1) || { echo "$out" | tail -5; fail "apply asks again after the confirmation"; }
pass
