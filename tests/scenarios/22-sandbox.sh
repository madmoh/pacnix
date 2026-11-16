#!/usr/bin/env bash
# Scenario 22: a PKGBUILD runs on the host in a sandbox. The global scope of a
# local package tries to write to /var/tmp, /tmp, /dev/shm and /etc, and to
# read a file that all users can read in a home directory. The build
# succeeds, the files outside the build directory do not occur on the host,
# and the content of the home file is not in the build directory.
source "$(dirname "$0")/../lib.sh"
fresh $h
on $h 'chmod 755 ~ && echo pacnix-secret >~/secret && chmod 644 ~/secret'
scenario $h '{ packages.local.escape = ./packages/escape; }'
out=$(apply $h) || { echo "$out" | tail -30; fail "the build of the package failed"; }
check $h 'pacman -Q escape' "the package is not installed"
for f in /var/tmp /tmp /dev/shm /etc; do
	refute $h "test -e $f/pacnix-escape" "the PKGBUILD wrote $f/pacnix-escape"
done
check $h 'sudo test -e /var/lib/pacnix/build/local/escape/leak' "the PKGBUILD did not run in the build directory"
refute $h 'sudo grep -rq pacnix-secret /var/lib/pacnix/build' "the PKGBUILD read the home directory"
pass
