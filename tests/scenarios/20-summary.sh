#!/usr/bin/env bash
# Scenario 20: the summary at the end of apply. The install notes come from the
# part of the pacman log that the run wrote: the output of a scriptlet goes to
# the package before it, two packages of one transaction keep their own notes,
# and the output of a hook is not a note. A note that did not change does not
# show again; a changed note does. A backup file that the user changed gives a
# .pacnew file in the summary. The audit shows the last notes.
# The install notes of the configuration: a note without a check shows in the
# same block as the notes of the packages, one time, again when its message
# changes and after its removal and addition, and not after an apply with a
# failed step. A note with a check shows while the check, which runs as its
# user with a clean environment, exits with status 1, also at a no-op and an
# offline apply; exit status 0 hides it. A check without a result (other exit
# status, time limit, missing program, unknown user, UID 0) shows one line and
# does not fail the apply. Plan and audit do not run the checks or show the notes.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
packages='packages.local.other = ./packages/other;'
welcome='installNotes.welcome.message = "Read the welcome text.";'

step "install two packages with notes in one transaction, with a note of the configuration"
scenario $h "{ packages.local.notes = ./packages/notes-1; $packages $welcome }"
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
log=$(on $h 'tail -60 /var/log/pacman.log')
echo "$log"
grep -A1 'installed notes (1-1)' <<<"$log" | grep -q 'ALPM-SCRIPTLET\] notes: first text' || fail "the pacman log does not put the note after its package"
grep -A1 'installed other (1-1)' <<<"$log" | grep -q 'ALPM-SCRIPTLET\] other: note of other' || fail "the pacman log does not put the note of other after other"
has "$out" '• Notes (pacnix audit shows the messages of the install scripts):' || { echo "$out" | tail; fail "the summary does not show one block of notes"; }
grep -qx '  Package notes: notes: first text' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the note of notes"; }
grep -qx '  Package other: other: note of other' <<<"$out" || fail "the summary does not show the note of other"
grep -qx '  welcome: Read the welcome text.' <<<"$out" || fail "the summary does not show the note of the configuration"
grep -q 'hook output of notes' <<<"$out" && fail "the summary shows the output of a hook as a note"
pacnix $h audit | grep -qx '• Install note of the last change: notes' || fail "the audit does not show the last notes"

step "an upgrade with the same note and a changed backup file"
on $h 'echo "# local change" | sudo tee -a /etc/notes.conf >/dev/null'
scenario $h "{ packages.local.notes = ./packages/notes-2; $packages $welcome }"
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
grep -q 'notes: \|welcome' <<<"$out" && fail "the summary shows a note that did not change"
grep -qx '• New file /etc/notes.conf.pacnew: merge it with /etc/notes.conf.' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the .pacnew file"; }

step "an upgrade with a changed note shows only the package note"
scenario $h "{ packages.local.notes = ./packages/notes-3; $packages $welcome }"
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
has "$out" '• New messages of the install scripts of declared packages (pacnix audit shows them again):' || { echo "$out" | tail; fail "the summary of the package notes changed"; }
grep -qx '  notes: notes: second text' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the changed note"; }
grep -q 'welcome' <<<"$out" && fail "the summary shows the note of the configuration again"
empty_plan $h

step "a note with a check shows while the check exits with status 1"
probe=$(cat <<'EOF'
files."/usr/local/bin/probe" = { mode = "0755"; text = ''
  #!/bin/sh
  { id -u; id -ru; id -G; echo "home=$HOME user=$USER"; env; } >/tmp/probe.out
  echo raw output of the check
  rc=$(cat /var/tmp/probe.rc)
  [ "$rc" = sleep ] && sleep 30
  exit "$rc"
''; };
EOF
)
check='installNotes.probe = { message = "Enroll the probe."; check = { user = "nobody"; command = [ "/usr/local/bin/probe" "an argument" ]; }; };'
base="packages.local.notes = ./packages/notes-3; $packages $probe"
rc() { on $h "echo $1 | sudo tee /var/tmp/probe.rc >/dev/null; sudo rm -f /tmp/probe.out"; }
rc 1
scenario $h "{ $base $welcome $check }"
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
grep -qx '  probe: Enroll the probe.' <<<"$out" || { echo "$out" | tail; fail "the summary does not show the note of a check with exit status 1"; }
grep -q 'welcome\|raw output' <<<"$out" && fail "the summary shows the note without a check again or the output of the check"
probe_out=$(on $h 'cat /tmp/probe.out') || fail "the check did not run after the changes"
[[ $(sed -n 1,3p <<<"$probe_out" | tr '\n' ' ') == "65534 65534 65534 " ]] || { echo "$probe_out"; fail "the check does not run as the user nobody with its groups"; }
grep -qx 'home=/ user=nobody' <<<"$probe_out" || { echo "$probe_out"; fail "the check does not have the HOME and USER of nobody"; }
grep -q '^SUDO_\|^SSH_\|^MAIL=' <<<"$probe_out" && { echo "$probe_out"; fail "the check has the environment of the user that runs pacnix"; }
rc 1
out=$(plan $h)
pacnix $h audit >>/dev/null || true
refute $h 'test -e /tmp/probe.out' "plan or audit ran the check"
grep -q 'probe\|welcome' <<<"$out$(pacnix $h audit)" && fail "plan or audit shows a note of the configuration"
out=$(apply $h) || fail "the no-op apply failed"
grep -q '^• Apply done. No changes.' <<<"$out" && grep -qx '  probe: Enroll the probe.' <<<"$out" || { echo "$out"; fail "the no-op apply does not show the note or counts it as a change"; }
rc 1
out=$(apply $h --offline) || fail "the offline apply failed"
grep -qx '  probe: Enroll the probe.' <<<"$out" || { echo "$out"; fail "the offline apply does not show the note"; }

step "exit status 0 hides the note, and it comes back when the check exits with status 1 again"
rc 0
out=$(apply $h) || fail "apply failed"
grep -q 'probe\|^• Notes' <<<"$out" && { echo "$out"; fail "the note of a done step shows"; }
check $h 'test -e /tmp/probe.out' "the check did not run"
rc 1
out=$(apply $h) || fail "apply failed"
grep -qx '  probe: Enroll the probe.' <<<"$out" || { echo "$out"; fail "the note does not come back"; }

step "a check without a result shows one line and the apply continues"
rc 3
out=$(apply $h) || { echo "$out"; fail "a check with exit status 3 failed the apply"; }
grep -qx '  probe: the check gave no result: the check ended with exit status 3' <<<"$out" || { echo "$out"; fail "the summary does not show the exit status of the check"; }
rc sleep
out=$(apply $h) || { echo "$out"; fail "a check that does not end failed the apply"; }
grep -qx '  probe: the check gave no result: the check did not end in 5 s' <<<"$out" || { echo "$out"; fail "the summary does not show the time limit"; }
on $h 'sudo useradd -o -u 0 -M -s /usr/bin/nologin toor'
broken='installNotes = {
  missing = { message = "Run the missing program."; check = { user = "nobody"; command = [ "/usr/local/bin/missing" ]; }; };
  stranger = { message = "Check as a stranger."; check = { user = "stranger"; command = [ "/usr/bin/true" ]; }; };
  alias = { message = "Check as root."; check = { user = "toor"; command = [ "/usr/bin/true" ]; }; };
};'
scenario $h "{ $base $welcome $broken }"
out=$(apply $h) || { echo "$out"; fail "the checks without a result failed the apply"; }
for l in 'missing: the check gave no result: /usr/local/bin/missing is not an executable file' \
	'stranger: the check gave no result: the user stranger does not exist' \
	'alias: the check gave no result: the user toor has the UID 0, and a check does not run as root'; do
	has "$out" "$l" || { echo "$out"; fail "the summary does not show: $l"; }
done
grep -q 'Enroll the probe' <<<"$out" && fail "the note of a removed declaration shows"
for bad in 'installNotes.bad.message = " ";' 'installNotes.bad = { message = "a"; check = { user = "nobody"; command = [ ]; }; };' \
	'installNotes.bad = { message = "a"; check = { user = "root"; command = [ "/usr/bin/true" ]; }; };'; do
	scenario $h "{ $bad }"
	out=$(plan $h) && fail "plan accepted: $bad"
	has "$out" 'installNotes.bad.' || { echo "$out"; fail "the error does not name the option: $bad"; }
done

step "a note without a check shows again after a change of its message, and after its removal and addition"
scenario $h "{ $base installNotes.welcome.message = \"Read the new welcome text.\"; }"
out=$(apply $h) || fail "apply failed"
grep -qx '  welcome: Read the new welcome text.' <<<"$out" || { echo "$out"; fail "the changed note does not show"; }
out=$(apply $h) || fail "apply failed"
grep -q 'welcome' <<<"$out" && fail "the changed note shows two times"
scenario $h "{ $base }"
apply $h >/dev/null || fail "apply failed"
scenario $h "{ $base installNotes.welcome.message = \"Read the new welcome text.\"; }"
out=$(apply $h) || fail "apply failed"
grep -qx '  welcome: Read the new welcome text.' <<<"$out" || { echo "$out"; fail "the note does not show after its removal and addition"; }

step "a note without a check waits for an apply without a failed step"
later='installNotes.later.message = "Use the new feature.";'
trigger() { scenario $h "{ $base $later files.\"/etc/notes-trigger\".text = \"1\"; triggers.fails = { paths = [ \"/etc/notes-trigger\" ]; command = \"$1\"; }; }"; }
trigger false
rc=0
out=$(apply $h) || rc=$?
((rc == 4)) || { echo "$out"; fail "the exit status is $rc, not 4"; }
grep -q 'later' <<<"$out" && fail "the note shows after a failed step"
trigger true
out=$(apply $h) || fail "apply failed"
grep -qx '  later: Use the new feature.' <<<"$out" || { echo "$out"; fail "the note does not show after the apply without a failed step"; }
pass
