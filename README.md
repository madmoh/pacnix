# pacnix

pacnix manages an Arch Linux system from a declarative configuration. You write the packages, users, mounts, boot loader, files, services and home configuration of each host in the Nix language. `pacnix apply` then makes the system agree with that configuration. pacman, makepkg and systemd make all the changes. The system stays a normal Arch Linux system.

pacnix changes only what the configuration declares. When the configuration declares nothing about an area, for example units, users or Secure Boot, pacnix does not change or check that area. A configuration with one package gives a plan with only that package. pacnix is idempotent and it reconciles: when you remove a declaration, pacnix removes what it made from that declaration, and nothing else. `pacnix audit` shows the state that the configuration does not declare.

## Installation

pacnix is in the signed pacman repository `[pacnix]`, which the releases of this project supply. Each release has the packages of pacnix, its signing key `pacnix-keyring` and [aurutils](https://github.com/aurutils/aurutils), the repository database and the signing key `pacnix.asc`. pacnix uses aurutils for the AUR, thus aurutils is a dependency of pacnix and comes built from `[pacnix]`. Each day, a job puts the latest aurutils release into the repository. To trust the key and add the repository, run these commands:

```sh
curl -fLO https://github.com/madmoh/pacnix/releases/latest/download/pacnix.asc
sudo pacman-key --add pacnix.asc
sudo pacman-key --lsign-key 94EE70A8B3EE6E0EE54639ABCE62ED9BA4742A16
printf '\n[pacnix]\nServer = https://github.com/madmoh/pacnix/releases/latest/download\n' | sudo tee -a /etc/pacman.conf
```

For more information, see [Unofficial user repositories](https://wiki.archlinux.org/title/Unofficial_user_repositories).

### Usage after the installation

pacnix runs on a booted system, because systemd makes many of its changes. In a chroot, for example `arch-chroot`, pacnix stops before it changes the system. Thus install Arch Linux with the [installation guide](https://wiki.archlinux.org/title/Installation_guide), with a boot loader, a user that can use `sudo`, and a network connection. The steps that you declare later, for example the time zone, the locale and the host name, can stay at their defaults. pacnix also takes control of a system that you used and configured before.

Boot the system, log in, add `[pacnix]` as the previous section shows, and install the package:

```sh
sudo pacman -Syu pacnix
```

pacnix uses the Nix store as root. It does not need the nix-daemon, and it does not change the nix-daemon units.

## First configuration

These commands make a configuration in `/etc/pacnix` for this host. It sets the time zone, installs Git, and gives your user a Git identity and a shell alias:

```sh
sudo install -d -o "$USER" /etc/pacnix
cat >/etc/pacnix/flake.nix <<EOF
{
  inputs.pacnix.url = "path:/usr/share/pacnix";
  outputs = { pacnix, ... }: {
    pacnixConfigurations.$(cat /etc/hostname) = pacnix.lib.pacnixSystem {
      modules = [ ./host.nix ];
    };
  };
}
EOF
cat >/etc/pacnix/host.nix <<EOF
{
  time.timeZone = "Europe/Berlin";
  packages.repo = [ "git" ];
  home.users.$USER = {
    programs.git.enable = true;
    programs.git.settings.user = { name = "Your Name"; email = "you@example.org"; };
    programs.bash.enable = true;
    programs.bash.shellAliases.ll = "ls -l";
  };
}
EOF
pacnix apply --no-upgrade --no-config
pacnix apply
```

`pacnix apply --no-upgrade --no-config` shows the plan and changes nothing. Change the configuration until the plan shows only the changes that you want, then run `pacnix apply`. Keep the configuration in a Git repository, for example to use it on a different host. When a declared file replaces a file that pacnix did not write, also a file of a package, pacnix keeps the original file in `/var/lib/pacnix/originals`, and the plan line tells this. A declared file that has the declared content already is no change. When you remove the declaration, pacnix puts the original file back. A declared user that exists keeps its password and its home directory. If you declare `/etc/pacman.conf`, keep the section `[pacnix]` in it.

The first apply builds and installs the AUR and local packages, and activates the home configurations. The builds can take a long time. When a step gives no output for 60 s, pacnix writes a line that tells that the step continues. A download shows the packages and MiB that arrived every 30 s, and tells when no data arrived for a minute.

## Configuration

The configuration is a [Nix flake](https://wiki.nixos.org/wiki/Flakes) in `/etc/pacnix`. Use `--flake` to use a different location. Each host is an output of the flake. By default, pacnix uses the host that has the name in `/etc/hostname`. Use `--host` to select a different host.

```nix
# /etc/pacnix/flake.nix
{
  inputs.pacnix.url = "path:/usr/share/pacnix";
  outputs = { pacnix, ... }: {
    pacnixConfigurations.myhost = pacnix.lib.pacnixSystem {
      modules = [ ./hosts/myhost.nix ./features/desktop.nix ];
    };
  };
}
```

Each file in `modules` is a module of the [NixOS module system](https://nixos.org/manual/nixos/stable/#sec-writing-modules). A good practice is one module for each feature (desktop, printing, development) and one module for each host. A host module selects the features and sets the values of that host. When two modules set conflicting values, the evaluation fails before pacnix changes the system.

```nix
# /etc/pacnix/hosts/myhost.nix
{
  packages.repo = [ "base" "linux-firmware" "pacnix" "networkmanager" "firefox" ];
  packages.aur = [ "visual-studio-code-bin" ];
  packages.local.my-tool = ../packages/my-tool;

  fileSystems."/" = { device = "LABEL=root"; fsType = "ext4"; };
  fileSystems."/boot" = { device = "LABEL=ESP"; fsType = "vfat"; options = [ "fmask=0077" "dmask=0077" ]; };
  zramSwap.enable = true;
  boot.kernels = [ "linux" ];
  boot.loader.systemd-boot.enable = true;

  networking.hostName = "myhost";
  time.timeZone = "Europe/Berlin";
  i18n.defaultLocale = "en_US.UTF-8";
  users.users.me = { extraGroups = [ "wheel" ]; shell = "/usr/bin/zsh"; };

  files."/etc/pacman.conf".source = ../pacman.conf;
  files."/etc/firefox/policies/policies.json".source = ./firefox-policies.json;

  units.enable = [ "NetworkManager.service" ];
  units.disable = [ "systemd-networkd.service" ];

  home.users.me = {
    dconf.settings."org/gnome/desktop/interface".color-scheme = "prefer-dark";
  };
}
```

### Packages

| Option | Function |
|---|---|
| `packages.repo` | Packages from the sync repositories. A [group](https://wiki.archlinux.org/title/Package_group) name, for example `gnome`, stands for all the members of the group. To select some members, declare the members. |
| `packages.aur` | Packages from the [AUR](https://wiki.archlinux.org/title/Arch_User_Repository). |
| `packages.local` | Directories that contain a [PKGBUILD](https://wiki.archlinux.org/title/PKGBUILD), by package name. |
| `packages.providers` | The package that satisfies a dependency, for example `{ jack = "pipewire-jack"; }`. This option has the same function as the provider choice of pacman. |
| `packages.versions` | An exact version for a declared package, for example `{ linux = "6.12.9.arch1-1"; }`. A repository package at a different version comes from the [Arch Linux Archive](https://wiki.archlinux.org/title/Arch_Linux_Archive). `pacnix apply` keeps the version. |
| `packages.server` | The URL of the repository of a different host. With this option, the host builds nothing. |
| `packages.serverKey` | The fingerprint of the key that signs the repository of `packages.server`. |
| `packages.serve.aur`, `packages.serve.local` | Packages that this host builds only for other hosts. |

After an apply, each declared package is explicitly installed. When a declaration goes, pacnix marks the package as a dependency, and pacman removes the packages that no other package needs. A package that was explicitly installed before its first declaration stays as it is. pacnix does not change the packages that no configuration declared, and the orphans that were installed before the apply stay. pacnix uses aurutils to build AUR and local packages in a [clean chroot](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot) with the settings of `/etc/makepkg.conf`, for example the compression, `MAKEFLAGS` and debug packages. Declare this file to change them, see [makepkg](https://wiki.archlinux.org/title/Makepkg). The packages go into the local repository `/var/lib/pacnix/repo`.

The PKGBUILD of a package runs on the host as the user `pacnix`, for example to download the sources, and in the clean chroot for the build. On the host, it runs in a private [mount namespace](https://man.archlinux.org/man/unshare.1): the file system is read-only, apart from the build directory `/var/lib/pacnix/build`, the home directories are not visible, and `/tmp`, `/var/tmp` and `/dev/shm` are private. The user `pacnix` has no sudo rule: root starts the clean chroot and adds the packages to the repository, which the user `pacnix` cannot change.

pacnix signs the local repository with a key that it makes at the first build and keeps in `/var/lib/pacnix/gnupg`. At that time, the output shows the fingerprint. To give the repository to other hosts, serve `/var/lib/pacnix/repo` with an HTTP server. On each of the other hosts, declare `packages.server` and the fingerprint in `packages.serverKey`. pacnix then downloads the public key `pacnix-local.asc` from the server, makes sure that it has the declared fingerprint, and trusts it with `pacman-key`. pacman installs only the packages that have a correct signature. To show the fingerprint again, run `gpg --show-keys /var/lib/pacnix/repo/pacnix-local.asc`.

Arch Linux does not support [partial upgrades](https://wiki.archlinux.org/title/System_maintenance#Partial_upgrades_are_unsupported). A pinned library can break the packages that need a newer version. An AUR package keeps the version in the local repository. For a different version, declare its PKGBUILD in `packages.local`.

When two declared packages conflict, pacnix refuses the plan. `pacnix apply` does not remove a package because of a conflict. If an upgrade needs such a removal, pacman stops, and you do the steps in the [Arch Linux news](https://archlinux.org/news/).

`pacnix apply` shows the plan, upgrades the system and then applies the configuration. The system upgrade runs `pacman -Syu` and builds the AUR packages again when necessary. To apply the configuration installs and removes the declared packages and makes the other changes. `pacnix apply --no-upgrade` does not upgrade the system. It still downloads and builds the packages that the configuration adds. `pacnix apply --no-config` upgrades the system with the configuration of the last apply. `--no-plan` does not show the plan.

Each AUR user can change an AUR package. At the first build of an AUR package, pacnix records its maintainer, its co-maintainers and its AUR dependencies in `/var/lib/pacnix`, without a question. When a later build shows a different maintainer or co-maintainer, or a new AUR dependency, `apply` shows a notice and continues. `pacnix audit` shows the notices of the last 30 days. To see the changes of a PKGBUILD before the build, run `pacnix apply --no-upgrade --no-config` and read the PKGBUILD on the [AUR](https://aur.archlinux.org/).

pacnix downloads the packages into the pacman cache before the transaction. If a mirror does not have a package version of the sync databases, the databases are old: run `pacnix apply` without `--no-upgrade`.

### Files

`files` contains files outside the home directories. Set one of these options:

- `text`: the content, exactly as written. pacnix adds no newline at the end, as [`environment.etc`](https://search.nixos.org/options?query=environment.etc) of NixOS. A Nix string with `''` adds the newline of its last line. For a final newline, end the text with `\n`.
- `source`: a file that has the content.
- `target`: the target of a symbolic link.

`mode`, `user` and `group` set the permissions. pacnix writes regular files with an atomic replace. Some file systems, for example vfat, cannot keep a mode or an owner, because the mount options set them for all files. In this condition, pacnix refuses the apply before it changes the system, and the error shows the mount options. Set `mode = null` to use the mode and the owner that the mount options give, for example for a file on the EFI system partition, or change the mount options in [fstab](https://wiki.archlinux.org/title/Fstab). When you remove a declaration of a file, pacnix deletes the file. If the file was there before pacnix, pacnix restores the original file. Else, if a package owns the file, pacnix restores the file of the package.

When possible, use the drop-in directories, for example `/etc/sysctl.d`, `/etc/systemd/system/*.d` and `/etc/default/grub.d`. Do not replace the complete file of a package.

Directories are files in [tmpfiles.d](https://man.archlinux.org/man/tmpfiles.d.5).

### Triggers

`triggers.<name>` runs `command` after a change to one of its `paths`. A path that ends with `/` includes all the files below it. Use triggers for commands such as `mkinitcpio -P`. For changes that a package makes, use a [pacman hook](https://man.archlinux.org/man/alpm-hooks.5).

### Users and groups

| Option | Function |
|---|---|
| `users.users.<name>` | A user account. pacnix creates a missing account with [useradd](https://man.archlinux.org/man/useradd.8) and changes an existing account with [usermod](https://man.archlinux.org/man/usermod.8). |
| `users.users.<name>.uid` | The user ID of a new account. If an existing account has a different ID, pacnix stops before it changes the system. |
| `users.users.<name>.group` | The primary group. By default, a new account gets a group with its name. |
| `users.users.<name>.extraGroups` | The supplementary groups, for example `[ "wheel" ]`. The account is in these groups and in no other supplementary group. |
| `users.users.<name>.shell` | The login shell, for example `/usr/bin/zsh`. Declare the package of the shell. |
| `users.users.<name>.home` | The home directory of a new account. The default is `/home/<name>`. |
| `users.users.<name>.description` | The full name (GECOS field). |
| `users.groups.<name>.gid` | A group, and the group ID of a new group. |

The configuration contains no passwords. Before it changes the system, `pacnix apply` asks for the password of each account that it creates, and gives it to [chpasswd](https://man.archlinux.org/man/chpasswd.8). pacnix does not keep the password. Without a terminal, the new account has no password, and the summary tells you to set it with `passwd`. When you remove the declaration of an account that pacnix created, pacnix deletes the account with `userdel`. The home directory stays. pacnix does not delete an account that it did not create.

### File systems and swap

| Option | Function |
|---|---|
| `fileSystems.<mount point>` | A file system to mount, with `device`, `fsType` and `options`, for example `{ device = "UUID=…"; fsType = "ext4"; }`. |
| `swapDevices` | Swap partitions and swap files, for example `[ { device = "/swapfile"; size = 4096; } ]`. pacnix makes a swap file with a `size` in MiB that does not exist with [mkswap](https://man.archlinux.org/man/mkswap.8). |
| `zramSwap.enable`, `zramSwap.settings` | Swap on compressed memory with [zram-generator](https://wiki.archlinux.org/title/Zram). |

With a declared file system or swap device, pacnix writes all of [/etc/fstab](https://wiki.archlinux.org/title/Fstab). Declare all the file systems of the system, also the root file system and the EFI system partition. A new mount is active after the next boot. pacnix writes the declared files below a declared mount point only when the declared file system is mounted there. When you remove the declaration of a swap file, the swap file is the data of a removed feature. pacnix does not make partitions or file systems.

### Boot

| Option | Function |
|---|---|
| `boot.kernels` | The kernel packages, for example `[ "linux" "linux-lts" ]`. The boot loader gets an entry for each kernel. |
| `boot.kernelParams` | The kernel command line, for example `[ "quiet" "loglevel=3" ]`. pacnix adds `root=`, `rw` and the `subvol=` option of `fileSystems."/"` before it, in `/etc/kernel/cmdline` and in the entries and UKIs of systemd-boot. GRUB gets only `kernelParams`, and `grub-mkconfig` adds the root parameters. |
| `boot.loader.efi.efiSysMountPoint` | The mount point of the [EFI system partition](https://wiki.archlinux.org/title/EFI_system_partition), `/boot` or `/efi`. The default is `/boot`. |
| `boot.loader.systemd-boot.enable`, `.settings` | [systemd-boot](https://wiki.archlinux.org/title/Systemd-boot), with the settings of [loader.conf](https://man.archlinux.org/man/loader.conf.5), for example `{ timeout = 3; }`. The first kernel is the default. Without UKIs, pacnix writes one entry for each kernel, and the EFI system partition must be at `/boot`. |
| `boot.loader.systemd-boot.uki` | Boot each kernel from a [unified kernel image](https://wiki.archlinux.org/title/Unified_kernel_image) `EFI/Linux/arch-KERNEL.efi` on the EFI system partition, with the command line in the image. Needs mkinitcpio. |
| `boot.loader.grub.enable`, `.device`, `.settings` | [GRUB](https://wiki.archlinux.org/title/GRUB). With `device`, for example `/dev/sda`, GRUB installs for BIOS on that disk. Without `device`, it installs on the EFI system partition. `settings` are variables of `/etc/default/grub`, for example `{ GRUB_TIMEOUT = "1"; }`. |

pacnix installs the boot loader when it is missing, and the boot loader adds itself to the EFI boot order. Enable one boot loader. pacnix does not remove a boot loader.

With [mkinitcpio](https://wiki.archlinux.org/title/Mkinitcpio), pacnix writes the preset of each kernel. The pacman hook of mkinitcpio builds the images after a kernel installation. pacnix runs the same hook after a change of a preset, of `/etc/mkinitcpio.conf`, of a file in `/etc/mkinitcpio.conf.d` or of the command line, then deletes the images of the other format. Declare the hooks in a file of `/etc/mkinitcpio.conf.d`, for example `sd-encrypt`. Install `amd-ucode` or `intel-ucode` for the microcode, which the `microcode` hook adds to the image.

pacnix writes files on the EFI system partition only when a vfat file system is mounted at its mount point. When the EFI system partition is at a different mount point, for example `/efi`, mount it at the declared mount point before the apply: `umount /efi` and `mount --mkdir /dev/disk/by-uuid/UUID /boot`. pacnix keeps the entries and images that it did not write. Delete the entries of other systems, for example `/boot/loader/entries/old.conf`, after the first boot of the new configuration.

systemd-boot shows Windows only when the Windows Boot Manager is on the same EFI system partition. For Windows on a different disk, use the boot menu of the firmware. The menu entry `Reboot Into Firmware Interface` of systemd-boot opens the firmware setup.

### Secure Boot and TPM2 unlock

| Option | Function |
|---|---|
| `boot.loader.systemd-boot.secureBoot.enable` | [Secure Boot](https://wiki.archlinux.org/title/Unified_Extensible_Firmware_Interface/Secure_Boot) with [sbctl](https://man.archlinux.org/man/sbctl.8) and the keys of this host. Needs systemd-boot. |
| `.dbAdditions` | The other keys and hashes that `sbctl enroll-keys` adds to the firmware: `microsoft`, `firmware-builtin`, `tpm-eventlog`. The default is `[ "microsoft" ]`, for the option ROMs of graphics cards and for Windows. Without the Microsoft keys, use `[ "tpm-eventlog" ]`: sbctl adds the hashes of the option ROMs that the firmware loaded. When sbctl refuses the keys because of an option ROM, the firmware stays in Setup Mode, and the summary shows the next step. pacnix does not ignore this check of sbctl. |
| `.extraFiles` | Other EFI binaries to sign, for example the kernel of an entry that pacnix did not write. pacnix also signs the files of the entries in `loader/entries`. |
| `boot.initrd.luks.devices.<name>.device` | The LUKS device of `/dev/mapper/<name>` in the initramfs, for example `UUID=…`. pacnix writes `/etc/crypttab.initramfs`. Needs mkinitcpio with the `sd-encrypt` hook. |
| `.options` | Other options of [crypttab](https://man.archlinux.org/man/crypttab.5), for example `[ "discard" ]`. |
| `.tpm2.enable`, `.tpm2.pcrs`, `.tpm2.pin` | Unlock the device with the TPM2 chip, through [systemd-cryptenroll](https://man.archlinux.org/man/systemd-cryptenroll.1). The default PCR list is `[ 7 ]`, the Secure Boot state. With `pin`, the initramfs asks for a PIN. |
| `.recoveryKey` | Enroll a recovery key once. |

pacnix creates the keys in `/var/lib/sbctl` when they are missing, and signs systemd-boot, the UKIs and the kernels of the entries. The pacman hook and the mkinitcpio hook of sbctl sign them again after each update. When a file that pacnix saved in the database of sbctl is no longer a boot file of the configuration, pacnix removes it from the database. The files that you save with `sbctl sign -s` stay. When the firmware is in Setup Mode, pacnix enrolls the keys. The keys do not go into the configuration.

Do these steps in the firmware setup. The summary of the apply tells the next step:

1. Set Secure Boot to custom mode, and reset it to Setup Mode. This deletes the factory keys.
2. Run `pacnix apply` in a terminal. It enrolls the keys of this host. With `recoveryKey`, it asks for the current passphrase of each device and shows the recovery key one time, in a separate block. Write down the recovery key, then type `written` to continue. If the input ends before `written`, `apply` stops and the key stays enrolled: if you do not have the key, remove it with `systemd-cryptenroll --wipe-slot=recovery`, then run `pacnix apply` to get a new key. When sbctl refuses the keys, the firmware stays in Setup Mode, and pacnix tells the correction from the message of sbctl: for example, add `tpm-eventlog` or `microsoft` to `dbAdditions` when the firmware loads option ROMs, or remove `tpm-eventlog` when the TPM event log has no option ROM.
3. Reboot, and turn on Secure Boot if it is off.
4. Run `pacnix apply` in a terminal. It asks for the current passphrase of each device and for a new PIN, then enrolls the TPM2 key.

The passphrase stays as a second key. When `tpm2.pcrs` contains 7, pacnix enrolls the TPM2 key only when the firmware checks the signatures, with the keys of this host when Secure Boot is declared. When the PCR values change, for example after new keys, the initramfs asks for the passphrase, and the next `pacnix apply` enrolls the TPM2 key again. When the firmware checks the signatures with other keys, pacnix stops before it changes the system. `pacnix audit` shows the Secure Boot state, the boot files without a signature and the state of each TPM2 key.

To remove Secure Boot, turn off Secure Boot in the firmware first. pacnix stops when the declaration goes while the firmware checks the signatures, because the next update then installs a boot file without a signature. When `tpm2.enable` goes, pacnix removes the TPM2 key. pacnix does not remove a recovery key.

### Host name, time zone and locale

| Option | Function |
|---|---|
| `networking.hostName` | The host name, in `/etc/hostname`. |
| `time.timeZone` | The time zone, for example `Europe/Berlin`. |
| `i18n.defaultLocale` | `LANG`, for example `en_US.UTF-8`. |
| `i18n.extraLocaleSettings` | Other variables of `/etc/locale.conf`, for example `{ LC_TIME = "en_GB.UTF-8"; }`. |
| `i18n.supportedLocales` | The lines of `/etc/locale.gen`. The default contains the default locale. pacnix runs `locale-gen` after a change. |
| `console.keyMap`, `console.font` | The console keyboard layout and font, in `/etc/vconsole.conf`. |

### Services

`units.enable`, `units.disable` and `units.mask` contain systemd system units. pacnix writes them as a [preset](https://man.archlinux.org/man/systemd.preset.5) file and runs `systemctl preset` for these units. When a unit goes from the configuration and pacnix changed its state, pacnix runs `systemctl preset` for it again, and its vendor preset decides its state. pacnix does not change the other units, also not a declared unit that was enabled before its declaration. pacnix starts a unit of `units.enable` when it enables the unit, and stops a unit when it disables the unit. pacnix does not start a unit that is enabled already, for example a unit that you stopped. Declare each unit that you need in `units.enable`, for example the display manager: pacnix does not enable the units of a new package through their vendor preset.

### Home

`home.users.<name>` contains a [Home Manager](https://nix-community.github.io/home-manager/) module. Use it for dotfiles, dconf settings and user units. pacnix activates it after the system step. Root builds the home configuration and writes its profile and its GC roots, and the activation runs as the user and only reads the Nix store. Thus the nix-daemon is not necessary.

Install software with `packages`, not with `home.packages`. An enabled program, for example `programs.git` or `programs.starship`, gets no package from Nix: pacnix sets its `package` option to `null`, or, when the option does not permit `null`, to a package that only links to the program of the same name in `/usr/bin`. Declare the program in `packages.repo`, as the first configuration does for Git.

When a home file of the configuration exists and Home Manager did not make it, pacnix keeps it as `FILE.pacnix-hm-backup`, and the plan shows it. When the declaration of the file goes, the original comes back. When an application replaces a home file with its own file, pacnix replaces that file at the next change of the home configuration and keeps the first original. Declare a file that an application writes, for example `~/.config/mimeapps.list`, in `/etc/xdg` or not at all.

### Scope

Declare the input that an application reads:

- configuration files
- policies, for example the [Firefox policies](https://mozilla.github.io/policy-templates/), also for extensions
- dconf keys
- drop-ins

Do not declare the data that an application writes, for example databases, tokens and caches. Add these paths to `claims` so that the audit does not show them. When an apply removes a feature from `claims`, the paths of that feature stay, and `pacnix audit` shows them as leftovers with a number. Before an apply removes a package, pacnix records what the package can leave: the paths of its [tmpfiles](https://man.archlinux.org/man/tmpfiles.d.5) rules, the `StateDirectory=`, `CacheDirectory=`, `LogsDirectory=` and `ConfigurationDirectory=` of its system and user units ([systemd.exec](https://man.archlinux.org/man/systemd.exec.5)), the system users of its [sysusers](https://man.archlinux.org/man/sysusers.d.5) files, its files in `/etc` and the names of the package, its programs and its desktop IDs. The audit shows the paths that this record explains as leftovers too, with the evidence, for example `(the owner is user postgres of removed package postgresql)`. When no path agrees with the record of a package, the audit deletes the record. The audit then asks which leftovers to delete: `a` for all, Enter or `n` for none, or the numbers and ranges, for example `1 3-5`. The leftovers that stay show again in the next audit. pacnix deletes a leftover with `rm -r` in the interactive shell of the user whose home directory holds it, else of root, thus an alias of `rm` in that shell applies, for example `alias rm='trash'`. pacnix does not delete a path that a package owns, or that a declared file or a different claim includes.

`audit.ignore` contains glob patterns of paths that the audit does not show, for example `[ "/var/lib/docker/**" ]`. Use it for paths that no feature owns and that pacnix must never delete. `claims` groups the paths by feature: when the feature goes, the audit offers to delete them.

pacnix does not manage these items:

- partitions and file systems: make them with the installation guide, and declare their mounts in `fileSystems`
- firmware settings, for example the Secure Boot mode and the EFI boot order
- secrets and passwords: pacnix asks for them at the run and does not keep them

## Usage

Run pacnix as your user. pacnix starts itself again with `sudo`, which asks for your password one time. pacnix evaluates the configuration as root, builds packages as the user `pacnix`, and activates each home configuration as its user. Thus no file in a home directory becomes the property of root. `pacnix --help` and `pacnix COMMAND --help` show the commands and options.

| Command | Function |
|---|---|
| `pacnix apply` | Shows the plan, upgrades the system, then makes it agree with the configuration. It also shows the Arch Linux news since the last upgrade. `--no-plan`, `--no-upgrade` and `--no-config` skip a stage. With `--no-upgrade --no-config`, it shows the plan and changes nothing. |
| `pacnix uninstall` | Removes pacnix, aurutils and Nix with pacman, and deletes the data, users and groups that they made. The declared files, units and packages stay. The Home Manager files become copies outside the Nix store. |
| `pacnix audit` | Shows the state that the configuration does not explain: files without an owner, changed package files, `.pacnew` files, explicitly installed packages that are not declared, enabled units that are not declared and that no vendor preset enables, and paths in home directories that no claim explains. It shows first if a reboot is necessary, the install notes of the last change and the notices of the last 30 days. Then it shows the number of items, the items of each kind, and what to do. Last, it shows the numbered leftovers of removed packages and features and asks which to delete. |

`apply` asks only for the secrets that the changes need, before it changes the system: the passwords of new accounts and the passphrases and PINs of LUKS devices. The configuration is the approval of each change. The changes of AUR packages and the Arch Linux news show as notices at the end of `apply`, and in `pacnix audit` for 30 days. `uninstall` asks before it changes the system. As with pacman, the answers of `audit` and `uninstall` come from the standard input, also without a terminal.

Each line that pacnix writes breaks between words before column 80. A line that starts with `pacnix:` is a step or a result; the indented lines under it are its items. An empty line separates the sections. When the standard output is a terminal and [`NO_COLOR`](https://no-color.org/) is not set, the names of items show in bold, errors in red, and a download or a long step shows one live progress line. Without a terminal, the output has the same lines without colors, and a download shows its progress every 30 s.

The plan shows each change with its item. With `--no-upgrade`, it also shows the packages that the local sync databases have in a newer version. Without `--no-upgrade`, `apply` refreshes the sync databases first, thus it can upgrade more packages than the plan shows. A change of a file shows its mode and owner when they change, and the target of a link. The Home Manager support files are the files that Home Manager writes also for an empty home configuration, for example `~/.cache/.keep`: the plan shows only their number.

`apply` shows each step with its items when the step starts. The output of pacman, makepkg and the other commands shows only when a command fails, with its last lines. A step that changes nothing, for example the evaluation, shows only when it takes 2 s or more. The host and the flake show only when you give `--host` or `--flake` with a value that is not the default. At the end, `apply` shows the items that you must know: if this run made a reboot necessary (the kernel that runs is not installed, a boot image changed, or a DKMS module was built for a kernel that does not run), the new messages of the install scripts of the declared packages, and the names of other packages with new messages (`pacnix audit` shows all messages), and new `.pacnew` files. The last line shows the result, the changes and the exit status. For each kind of change, it names the items when there are three or fewer, and else it shows their number and the declared packages among them:

| Exit status | Result |
|---|---|
| 0 | The system agrees with the configuration. |
| 1 | pacnix stopped, for example at an error in the configuration or at a refused plan. The last line tells if a change was done before the stop. |
| 4 | The apply is done, but some steps failed. The next apply does them again. |
| 130 | You pressed Ctrl-C. Before the first change, pacnix stops at once. After it, pacnix stops after the current step, and the last line names that step. The next apply does the rest. A second Ctrl-C stops pacnix at once. |

An error in the configuration shows the message of Nix one time, with the file in the flake and its line.

When there are no new [Arch Linux news](https://archlinux.org/news/), `apply` shows nothing about them. When the news are not available, `apply` continues without them. The next apply that gets them shows the news since the last read.

When a download fails because of the network, for example no connection, a DNS timeout or a mirror that stops, pacnix tries again after 10 s, 30 s and 60 s, with one download at a time. Your `ParallelDownloads` setting in `/etc/pacman.conf` does not change. The next attempt uses the packages that are already in the pacman cache and in the local repository, and the downloaded sources of a local package stay until it builds.

A failed step that changes only one item does not stop `apply`, as with [`nix build --keep-going`](https://nix.dev/manual/nix/stable/command-ref/new-cli/nix3-build) and [`switch-to-configuration`](https://nixos.org/manual/nixos/stable/#sec-switching-systems) of NixOS. These steps are:

- the build of an AUR or a local package. The packages that need it do not build or install. The package removals wait until each declared package builds, because a package that did not build can replace a package that leaves. The units and files that leave go: when a package that did not build replaces a service, for example a display manager, the old service stops.
- a trigger. The later triggers wait, because a trigger can need the result of an earlier trigger, for example the boot loader needs the boot images.
- a `systemctl` command that starts, stops, restarts, masks or unmasks a unit
- the activation of a home configuration

pacnix shows the reason of each failed step when it fails, and applies the rest of the configuration. The last line shows the number of failed steps, and the exit status is 4. The next `apply` does the failed steps again, and its plan shows them. A failed `systemctl` command runs again only while the configuration wants its result: for example, when you remove a unit from `units.enable`, pacnix does not start it again. A failure in the network stops a step only after the last attempt. Each other failure, for example an error in the configuration, a refused plan or a pacman transaction that fails, stops pacnix with the reason and the exit status 1.

`apply` never removes pacnix and the packages that it needs: `pacnix uninstall` does. Each apply keeps a generation in the Nix profile `/nix/var/nix/profiles/pacnix`.

## See also

- [Arch Linux installation guide](https://wiki.archlinux.org/title/Installation_guide)
- [pacman](https://wiki.archlinux.org/title/Pacman) and [makepkg](https://wiki.archlinux.org/title/Makepkg)
- [systemd](https://wiki.archlinux.org/title/Systemd)
- [Nix language](https://nix.dev/manual/nix/stable/language/)
- [NixOS module system](https://nixos.org/manual/nixos/stable/#sec-writing-modules)
- [Home Manager options](https://nix-community.github.io/home-manager/options.xhtml)
