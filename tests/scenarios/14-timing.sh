#!/usr/bin/env bash
# Time cost of pacnix over plain pacman: a no-op apply against a no-op
# pacman -Syu, and one package added and removed through pacnix and through
# pacman. Prints seconds; the packages are in the pacman cache for both runs,
# so downloads do not count.
size=(2 3072 1)
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
t() { local s; s=$(date +%s.%N); "$@" >/dev/null || fail "$*"; awk -v s="$s" -v e="$(date +%s.%N)" 'BEGIN { print e - s }'; }
on $h 'sudo pacman -Sw --noconfirm biber >/dev/null'
step "measure"
pac_syu=$(t on $h 'sudo pacman -Syu --noconfirm')
noop=$(t apply $h)
offline=$(t apply $h --offline)
plan_s=$(t plan $h)
pac_add=$(t on $h 'sudo pacman -S --noconfirm biber')
pac_rm=$(t on $h 'sudo pacman -Rns --noconfirm biber')
scenario $h '{ packages.repo = [ "biber" ]; }'
nix_add=$(t apply $h)
scenario $h ''
nix_rm=$(t apply $h)
printf 'no-op: pacman -Syu %.1f s, pacnix apply %.1f s, pacnix apply --offline %.1f s, plan %.1f s\n' "$pac_syu" "$noop" "$offline" "$plan_s"
printf 'add biber: pacman %.1f s, pacnix apply %.1f s\n' "$pac_add" "$nix_add"
printf 'remove biber: pacman %.1f s, pacnix apply %.1f s\n' "$pac_rm" "$nix_rm"
pass
