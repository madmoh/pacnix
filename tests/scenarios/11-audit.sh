#!/usr/bin/env bash
# Scenario 11: a feature creates state, the feature goes, the audit lists the
# leftovers with the feature that claimed them.
source "$(dirname "$0")/../lib.sh"
fresh $h
scenario $h '{ features.demo.enable = true; }'
apply $h >/dev/null || fail "apply failed"
on $h 'mkdir -p ~/.cache/demo && curl -fs -o ~/.cache/demo/page localhost:8081'
step "the audit of a configured system"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
grep -q '/srv/demo\|\.cache/demo' <<<"$out" && fail "the audit lists claimed paths"
step "remove the feature"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
grep -qE '^unowned directory: /srv/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)$' <<<"$out" || fail "the audit does not list /srv/demo with its origin"
grep -qE '^unclaimed home path: /home/arch/.cache/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)$' <<<"$out" || fail "the audit does not list ~/.cache/demo with its origin"
pass
