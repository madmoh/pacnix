#!/usr/bin/env bash
# Scenario 3: add and remove a repo package, a group and AUR packages, remove a whole
# feature, and restore the original files after an override is dropped. A new AUR
# dependency of a built AUR package and package removals are in the one
# question of a run, which needs --yes without a terminal. The output of aur sync
# does not show when the build succeeds. After the install, the AUR checkouts
# keep only their git history, PKGBUILD and .SRCINFO, and no copy of the build
# chroot stays. A package base in packages.keepBuildCache keeps its sources until
# it leaves the list. The apply removes the sources that an earlier version of
# pacnix kept, and removes a link in the build directory, not its target.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
repo='pacman -Slq --config /run/pacnix/pacman.conf pacnix-local'

step "add a repo package and AUR packages, one with an AUR dependency"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; }'
out=$(apply $h) || { echo "$out"; fail "apply failed"; }
grep -q 'packages ignored' <<<"$out" && fail "apply shows the list of aur sync"
check $h "pacman -Q htop downgrade perl-email-outlook-message" "declared packages are missing"
check $h "pacman -Qqd perl-ole-storage-lite" "the AUR dependency is not installed as a dependency"
check $h "$repo | grep -qx perl-ole-storage-lite" "the AUR dependency is not in the local repository"
ckout="sudo ls -A /var/lib/pacnix/build/.cache/aurutils/sync"
# A checkout without files that git does not track: the sources and logs went.
clean() { check $h "test -z \"\$(sudo -u pacnix git -C /var/lib/pacnix/build/.cache/aurutils/sync/$1 status --porcelain --ignored)\" && $ckout/$1 | grep -qx PKGBUILD" "$2"; }
for b in downgrade perl-email-outlook-message perl-ole-storage-lite; do
	clean $b "the checkout of $b keeps its sources or logs"
done
check $h 'test "$(sudo ls /var/lib/aurbuild/x86_64 | tr "\n" " ")" = "root root.lock "' "a copy of the build chroot stays"
packages_match $h

step "a new AUR dependency of a built AUR package needs a confirmation"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; packages.keepBuildCache = [ "perl-email-outlook-message" ]; }'
on $h "sudo jq '.\"perl-email-outlook-message\".depends = []' /var/lib/pacnix/aur.json | sudo tee /var/lib/pacnix/aur.json.new >/dev/null && sudo mv /var/lib/pacnix/aur.json.new /var/lib/pacnix/aur.json && sudo GNUPGHOME=/var/lib/pacnix/gnupg repo-remove -s -q /var/lib/pacnix/repo/pacnix-local.db.tar.gz perl-email-outlook-message"
out=$(pacnix $h apply 2>&1) && fail "apply built a package with a new AUR dependency without a confirmation"
has "$out" 'The AUR package perl-email-outlook-message has new AUR dependencies: perl-ole-storage-lite.' || { echo "$out" | tail -5; fail "apply does not show the new AUR dependency"; }
out=$(apply $h) || { echo "$out"; fail "apply --yes failed"; }
grep -q 'packages ignored' <<<"$out" && fail "apply shows the list of aur sync"
check $h "$ckout/perl-email-outlook-message | grep -q '^Email-Outlook-Message-.*tar.gz$'" "the sources of a package in keepBuildCache do not stay"

step "the sources go when the package base leaves keepBuildCache, and old sources go"
on $h 'sudo -u pacnix sh -c "cd /var/lib/pacnix/build && touch .cache/aurutils/sync/downgrade/old.tar.gz && mkdir -p local/old sources/old && touch sources/old/big" &&
	sudo mkdir -p /srv/target && sudo touch /srv/target/file && sudo ln -s /srv/target /var/lib/pacnix/build/sources/link && sudo rm /var/lib/pacnix/build-cache.json'
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; }'
apply $h >/dev/null || fail "apply failed"
for b in downgrade perl-email-outlook-message; do
	clean $b "the sources of $b stay"
done
check $h 'test -z "$(sudo ls -A /var/lib/pacnix/build/local /var/lib/pacnix/build/sources | grep -v "^/\|^$")"' "old sources or a link stay"
check $h 'test -f /srv/target/file' "the apply removed the target of a link"
before=$(on $h 'stat -c %Y /var/lib/pacnix/build-cache.json')
empty_plan $h
apply $h >/dev/null || fail "the no-op apply failed"
[[ $before == $(on $h 'stat -c %Y /var/lib/pacnix/build-cache.json') ]] || fail "the no-op apply wrote the record of the sources"

step "remove them"
scenario $h ''
out=$(pacnix $h apply 2>&1) && fail "apply removed packages without a confirmation"
matches "$out" 'Remove [0-9]+ packages that are no longer declared and that pacnix installed, with the dependencies that only they need: .*downgrade.*htop' || { echo "$out" | tail -5; fail "apply does not list the removals in the question"; }
has "$out" '• Error: these items need a confirmation. Give --yes to continue without a terminal' || fail "apply without a terminal does not ask for --yes"
check $h "pacman -Q htop" "apply changed the system before the confirmation"
apply $h || fail "apply failed"
for p in htop downgrade perl-email-outlook-message perl-ole-storage-lite; do
	refute $h "pacman -Q $p" "$p is still installed"
	refute $h "$repo | grep -qx $p" "$p is still in the local repository"
	refute $h "ls /var/lib/pacnix/repo/$p-[0-9]*" "the file of $p stays in the local repository"
done
packages_match $h

step "a group stands for its members, as with pacman -S"
scenario $h '{ packages.repo = [ "fprint" ]; }'
apply $h >/dev/null || fail "apply failed"
check $h "pacman -Qqe fprintd libfprint" "the members of the group are not explicitly installed"
empty_plan $h
pacnix $h audit | grep -q '^    fprint' && fail "the audit does not count the members as declared"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h "pacman -Q fprintd" "a member of the removed group is still installed"

step "a symbolic link: create, change the target, drop"
scenario $h '{ files."/etc/pacnix-zone".target = "/usr/share/zoneinfo/UTC"; }'
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/pacnix-zone)" = /usr/share/zoneinfo/UTC' "the link is not created"
scenario $h '{ files."/etc/pacnix-zone".target = "/usr/share/zoneinfo/Europe/Berlin"; }'
changes "$(plan $h)" | grep -qx 'Changing file /etc/pacnix-zone (link to /usr/share/zoneinfo/Europe/Berlin)' || fail "the plan does not update the link"
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/pacnix-zone)" = /usr/share/zoneinfo/Europe/Berlin' "the link target is not changed"
empty_plan $h
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h 'test -L /etc/pacnix-zone' "the dropped link remains"

step "enable a feature"
scenario $h '{ features.demo.enable = true; }'
apply $h || fail "apply failed"
check $h "curl -fs localhost:8081" "the demo service does not answer"
check $h "systemctl show -p Environment sshd | grep -q DEMO=1" "the sshd drop-in is not active"
check $h "test \$(sysctl -n net.core.somaxconn) = 8192" "the sysctl trigger did not run"

step "remove the feature"
scenario $h ''
apply $h || fail "apply failed"
refute $h "pacman -Q darkhttpd" "darkhttpd is still installed"
for f in /etc/systemd/system/demo-web.service /etc/systemd/system/sshd.service.d /etc/sysctl.d/90-demo.conf; do
	refute $h "test -e $f" "$f still exists"
done
refute $h "systemctl is-enabled demo-web.service" "demo-web.service is still enabled"
refute $h "systemctl is-active demo-web.service" "demo-web.service is still running"
refute $h "find /etc/systemd/system -xtype l | grep ." "dangling unit links remain"
refute $h "systemctl show -p Environment sshd | grep -q DEMO=1" "the sshd drop-in is still active"
empty_plan $h

step "override package files"
originals=$(on $h 'md5sum /etc/pacman.conf /etc/locale.gen /etc/pam.d/su')
scenario $h '{ features.overrides.enable = true; }'
apply $h || fail "apply failed"
check $h "grep -qx Color /etc/pacman.conf" "pacman.conf is not overridden"
check $h "locale -a | grep -q ar_AE" "the locale trigger did not run"
check $h "grep -q '^auth.*pam_wheel.so use_uid' /etc/pam.d/su" "the PAM file is not overridden"
empty_plan $h

step "drop the overrides and restore the original files"
scenario $h ''
apply $h || fail "apply failed"
[[ $(on $h 'md5sum /etc/pacman.conf /etc/locale.gen /etc/pam.d/su') == "$originals" ]] || fail "the original files did not come back"
refute $h "locale -a | grep -q ar_AE" "the locale trigger did not run after the restore"
empty_plan $h
pass
