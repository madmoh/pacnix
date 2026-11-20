#!/usr/bin/env bash
# Scenario 11: a feature creates state, then the feature goes. The one question
# of the run offers to delete the data of the feature. The answer k keeps it,
# and the audit lists it with the feature that claimed it. --yes deletes it,
# except a declared file and a path that a feature of the configuration claims.
source "$(dirname "$0")/../lib.sh"
fresh $h
demo='{ features.demo.enable = true; }'
scenario $h "$demo"
apply $h >/dev/null || fail "apply failed"
on $h 'mkdir -p ~/.cache/demo/keep && curl -fs -o ~/.cache/demo/page localhost:8081'
step "the audit of a configured system"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
grep -q '/srv/demo\|\.cache/demo' <<<"$out" && fail "the audit lists claimed paths"
step "remove the feature and keep its data"
scenario $h ''
out=$(on $h "printf 'k\n' | script -qec 'NO_COLOR=1 pacnix apply' /dev/null" 2>&1) || { echo "$out" | tail; fail "apply with the answer k failed"; }
has "$out" 'Delete the data of removed feature demo: /srv/demo, /home/arch/.cache/demo.' || { echo "$out" | tail; fail "the question does not show the data of the feature"; }
grep -qF 'Continue? [y/N] (N stops pacnix, k continues and keeps the data)' <<<"$out" || fail "the question does not offer to keep the data"
grep -q '^• the data of the removed features stays. pacnix audit shows it.' <<<"$out" || fail "the summary does not tell that the data stays"
check $h 'test -e /srv/demo/index.html && test -e ~/.cache/demo/page' "the answer k deleted the data"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
matches "$out" 'unowned directory \([0-9]+\):.* /srv/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)' || fail "the audit does not list /srv/demo with its origin"
matches "$out" 'unclaimed home path \([0-9]+\):.* /home/arch/.cache/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)' || fail "the audit does not list ~/.cache/demo with its origin"
step "remove the feature again and delete its data"
scenario $h "$demo"
apply $h >/dev/null || fail "apply failed"
scenario $h '{ files."/srv/demo/keep.txt".text = "kept\n"; claims.other.home = [ ".cache/demo/keep" ]; }'
apply $h || fail "apply --yes failed"
refute $h 'test -e /srv/demo/index.html || test -e ~/.cache/demo/page' "the data of the removed feature remains"
check $h 'test "$(cat /srv/demo/keep.txt)" = kept && test -d ~/.cache/demo/keep' "a declared file or a claimed path is deleted"
pass
