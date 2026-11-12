#!/usr/bin/env bash
# Scenario 12: the plan of the package lists of a typical workstation, about
# 380 packages with 50 from the AUR. The plan resolves every repository and
# AUR name and the AUR dependency graph. Nothing is built.
source "$(dirname "$0")/../lib.sh"
step "plan the package lists of a workstation"
fresh $h
s=$(date +%s)
out=$(pacnix $h plan --host workstation) || { echo "$out" | tail; fail "plan failed"; }
echo "plan took $(($(date +%s) - s)) s"
grep '^unknown package' <<<"$out" && fail "the plan has unknown packages"
echo "AUR builds: $(grep -c '^build AUR package' <<<"$out"), packages to install: $(grep -c '^install package' <<<"$out")"
pass
