#!/usr/bin/env bash
# Scenario 20: the summary at the end of apply. The install notes come from the
# part of the pacman log that the run wrote: the output of a scriptlet goes to
# the package before it, two packages of one transaction keep their own notes,
# and the output of a hook is not a note. A note that did not change does not
# show again; a changed note does. A backup file that the user changed gives a
# .pacnew file in the summary. The audit shows the last notes.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"

step "install two packages with notes in one transaction"
scenario $h '{ packages.local.notes = ./packages/notes-1; packages.local.other = ./packages/other; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
log=$(on $h 'tail -60 /var/log/pacman.log')
echo "$log"
grep -A1 'installed notes (1-1)' <<<"$log" | grep -q 'ALPM-SCRIPTLET\] notes: first text' || fail "the pacman log does not put the note after its package"
grep -A1 'installed other (1-1)' <<<"$log" | grep -q 'ALPM-SCRIPTLET\] other: note of other' || fail "the pacman log does not put the note of other after other"
has "$out" 'pacnix: new messages of the install scripts of declared packages (pacnix audit shows them again):' || { echo "$out" | tail; fail "the summary does not show the notes of the declared packages"; }
grep -qx '  notes: notes: first text' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the note of notes"; }
grep -qx '  other: other: note of other' <<<"$out" || fail "the summary does not show the note of other"
grep -qx '  hook output of notes' <<<"$out" && fail "the summary shows the output of a hook as a note"
pacnix $h audit | grep -qx 'pacnix: install note of the last change: notes' || fail "the audit does not show the last notes"

step "an upgrade with the same note and a changed backup file"
on $h 'echo "# local change" | sudo tee -a /etc/notes.conf >/dev/null'
scenario $h '{ packages.local.notes = ./packages/notes-2; packages.local.other = ./packages/other; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
grep -q '^  notes: ' <<<"$out" && fail "the summary shows a note that did not change"
grep -qx 'pacnix: new file /etc/notes.conf.pacnew: merge it with /etc/notes.conf' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the .pacnew file"; }

step "an upgrade with a changed note"
scenario $h '{ packages.local.notes = ./packages/notes-3; packages.local.other = ./packages/other; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
grep -qx '  notes: notes: second text' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the changed note"; }
empty_plan $h
pass
