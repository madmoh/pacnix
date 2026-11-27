#!/usr/bin/env bash
# Scenario 28: an earlier release of pacnix sets up a host with the
# configuration of that release. The host then takes the current pacnix, which
# applies the same configuration, and then the current configuration. The
# current pacnix reads the records of the earlier release:
# when the declarations go, the package, the unit, the mask and the file that
# the earlier release made go too. An option that the configuration renames
# works and shows a warning. PACNIX_FROM selects the release, by default the
# latest release before this commit.
size=(4 4096 1)
source "$(dirname "$0")/../lib.sh"
from=${PACNIX_FROM:-$(git -C "$root" describe --tags --abbrev=0 HEAD^)}
# The earlier release, built from its tag and signed as in tests/package, in a
# repository [pacnix] with the aurutils of the test repository.
old=$(mktemp -d -p "$cache")
(
	git -C "$root" archive "$from" | tar -x -C "$old" && cd "$old" &&
	BUILDDIR=$old/build PKGDEST=$old/repo PACNIX_KEY=$cache/pkg/test-key.asc makepkg -df --noconfirm >"$old/log" 2>&1 &&
	cp "$cache"/pkg/aurutils-*.pkg.tar.zst "$cache/pkg/test-key.asc" repo/ &&
	export GNUPGHOME=$cache/gnupg && key=$(gpg --list-secret-keys --with-colons | awk -F: '/^fpr/ { print $10; exit }') &&
	for p in repo/*.pkg.tar.zst; do gpg --batch --yes --detach-sign --local-user "$key" "$p"; done &&
	repo-add -q --sign --key "$key" repo/pacnix.db.tar.gz repo/*.pkg.tar.zst
) &
build=$!
fresh $h "$cache/base-20261101.620452.qcow2"
wait $build || { tail -20 "$old/log"; rm -rf "$old"; fail "pacnix $from did not build"; }

step "set up the host with pacnix $from and its configuration"
tar -C "$old" -c repo tests/config fixtures | on $h 'mkdir -p old && tar -x -C old'
rm -rf "$old"
on $h 'sudo rsync -a --delete old/repo/ /srv/pacnix/ && sudo pacman -Sy --config /tmp/pacnix.conf >/dev/null &&
	sudo pacman -U --noconfirm /srv/pacnix/pacnix-*.pkg.tar.zst >/dev/null &&
	sudo rsync -a --delete --exclude scenario.nix old/tests/config/ /etc/pacnix/ && sudo rsync -a --delete old/fixtures/ /etc/pacnix/fixtures/'
# The hosts of the earlier flake end before this scenario: the current flake lists them.
on $h 'sudo cp ~/config/flake.nix /etc/pacnix/flake.nix'
scenario $h '{ packages.repo = [ "htop" ]; units.enable = [ "fstrim.timer" ]; units.mask = [ "hibernate.target" ]; files."/etc/pacnix-old".text = "old"; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "the apply of pacnix $from failed"; }
check $h 'pacman -Qqe htop' "pacnix $from did not install htop"
check $h 'test "$(systemctl is-enabled fstrim.timer)" = enabled' "pacnix $from did not enable fstrim.timer"
check $h 'test "$(systemctl is-enabled hibernate.target)" = masked' "pacnix $from did not mask hibernate.target"
check $h 'test -f /etc/pacnix-old' "pacnix $from did not make /etc/pacnix-old"

step "take the current pacnix, then the current configuration"
# The database of the test repository is older than the synced database of the earlier release.
on $h 'sudo rsync -a --delete ~/repo/ /srv/pacnix/ && sudo pacman -Syy --config /tmp/pacnix.conf >/dev/null &&
	sudo pacman -U --noconfirm /srv/pacnix/pacnix-*.pkg.tar.zst >/dev/null'
# As after an apply that upgrades pacnix: the current pacnix evaluates the configuration of the earlier release.
out=$(apply $h) || { echo "$out" | tail -20; fail "the current pacnix failed with the configuration of pacnix $from"; }
on $h 'sudo rsync -a --delete --exclude scenario.nix ~/config/ /etc/pacnix/'
out=$(apply $h) || { echo "$out" | tail -20; fail "the apply of the current configuration failed"; }
empty_plan $h

step "remove the declarations that pacnix $from applied"
scenario $h ''
out=$(apply $h) || { echo "$out" | tail -20; fail "the apply without the declarations failed"; }
refute $h 'pacman -Q htop' "the package that pacnix $from installed stays"
check $h 'test "$(systemctl is-enabled fstrim.timer)" = disabled' "the unit that pacnix $from enabled stays enabled"
refute $h 'test "$(systemctl is-enabled hibernate.target)" = masked' "the mask that pacnix $from made stays"
refute $h 'test -e /etc/pacnix-old' "the file that pacnix $from made stays"
packages_match $h
empty_plan $h

step "a renamed option works and shows a warning in apply and audit"
scenario $h '{ lib, ... }: { imports = [ (lib.mkRenamedOptionModule [ "oldRepo" ] [ "packages" "repo" ]) ]; oldRepo = [ "htop" ]; }'
warning="• Warning: The option \`oldRepo' defined in \`/etc/pacnix/scenario.nix' has been renamed to \`packages.repo'."
out=$(apply $h) || { echo "$out" | tail -20; fail "the apply with a renamed option failed"; }
once "$out" "$warning" || { echo "$out" | tail -20; fail "the apply does not show the warning of the renamed option one time"; }
check $h 'pacman -Qqe htop' "the renamed option did not install htop"
has "$(pacnix $h audit </dev/null)" "$warning" || fail "the audit does not show the warning of the renamed option"
pass
