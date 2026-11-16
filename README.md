# pacnix

pacnix manages an Arch Linux system from a declarative configuration. You write the packages, files, services and home configuration of each host in the Nix language. `pacnix apply` then makes the system agree with that configuration. pacman, makepkg and systemd make all the changes. The system stays a normal Arch Linux system.

pacnix is idempotent and it reconciles: when you remove a declaration, its package, file or service also leaves the system. When you read the configuration, you can find the cause of each item on the system.

## Installation

pacnix is in the signed pacman repository `[pacnix]`, which the releases of this project supply. Each release has the packages of pacnix and [aurutils](https://github.com/aurutils/aurutils), the repository database and the signing key `pacnix.asc`. pacnix uses aurutils for the AUR, thus aurutils is a dependency of pacnix and comes built from `[pacnix]`. Each day, a job puts the latest aurutils release into the repository. To trust the key and add the repository, run these commands as root. During an Arch Linux installation, run them on the live system.

```sh
curl -fLO https://github.com/madmoh/pacnix/releases/latest/download/pacnix.asc
pacman-key --add pacnix.asc
pacman-key --lsign-key 94EE70A8B3EE6E0EE54639ABCE62ED9BA4742A16
printf '\n[pacnix]\nServer = https://github.com/madmoh/pacnix/releases/latest/download\n' >> /etc/pacman.conf
```

For more information, see [Unofficial user repositories](https://wiki.archlinux.org/title/Unofficial_user_repositories).

### During the installation of Arch Linux

Do the steps of the [installation guide](https://wiki.archlinux.org/title/Installation_guide) until the partitions are mounted on `/mnt`. Then:

```sh
pacstrap -K /mnt base pacnix
cp /etc/pacman.conf /mnt/etc/pacman.conf
cp pacnix.asc /mnt/root/
arch-chroot /mnt pacman-key --add /root/pacnix.asc
arch-chroot /mnt pacman-key --lsign-key 94EE70A8B3EE6E0EE54639ABCE62ED9BA4742A16
cp -a config /mnt/etc/pacnix
arch-chroot /mnt pacnix apply --host myhost
```

In the chroot, you are root. `pacstrap -K` gives the new system an empty keyring, thus the key is added again in the chroot. If `pacstrap` fails, for example because a mirror stops, run it again: the packages that it downloaded stay in the pacman cache of `/mnt`. The apply in the chroot does these steps:

- It installs the packages from the repositories.
- It writes the files and enables the services.
- It runs the triggers, for example `locale-gen` and the installation of the boot loader.

A chroot cannot build AUR packages or activate a home configuration. At the end, the apply in the chroot shows the packages and home configurations that wait for the booted system, and the next command. `arch-chroot` mounts `/etc/resolv.conf` of the live system, thus pacnix cannot write a declared `/etc/resolv.conf` in the chroot. Write it after you leave the chroot, as the output shows, for example `ln -sf /run/systemd/resolve/stub-resolv.conf /mnt/etc/resolv.conf` for [systemd-resolved](https://wiki.archlinux.org/title/Systemd-resolved). Set the passwords with `passwd` before you reboot. pacnix does not manage secrets.

Reboot and log in. Connect to the network, for example to a WiFi network in the desktop. Then run:

```sh
pacnix upgrade
```

The upgrade refreshes the sync databases, builds and installs the AUR and local packages, and activates the home configurations. The builds can take a long time. When a step gives no output for 60 s, pacnix writes a line that tells that the step continues. A new installation asks no question.

### On an installed system

Install the package with `pacman -Syu pacnix`. Write the configuration in `/etc/pacnix`. Then run `pacnix plan` and change the configuration until the plan shows only the changes that you want. Then run `pacnix apply`.

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
  packages.repo = [ "base" "linux" "linux-firmware" "pacnix" "grub" "networkmanager" "firefox" ];
  packages.aur = [ "visual-studio-code-bin" ];
  packages.local.my-tool = ../packages/my-tool;

  files."/etc/hostname".text = "myhost\n";
  files."/etc/pacman.conf".source = ../pacman.conf;
  files."/etc/localtime".target = "/usr/share/zoneinfo/Europe/Berlin";
  files."/etc/firefox/policies/policies.json".source = ./firefox-policies.json;

  units.enable = [ "NetworkManager.service" ];
  units.disable = [ "systemd-networkd.service" ];

  triggers.locale = { paths = [ "/etc/locale.gen" ]; command = "locale-gen"; };

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
| `packages.versions` | An exact version for a declared package, for example `{ linux = "6.12.9.arch1-1"; }`. A repository package at a different version comes from the [Arch Linux Archive](https://wiki.archlinux.org/title/Arch_Linux_Archive). `pacnix upgrade` keeps the version. |
| `packages.server` | The URL of the repository of a different host. With this option, the host builds nothing. |
| `packages.serverKey` | The fingerprint of the key that signs the repository of `packages.server`. |
| `packages.serve.aur`, `packages.serve.local` | Packages that this host builds only for other hosts. |

After an apply, the explicitly installed packages are equal to the declared packages, and no orphan packages stay installed. pacnix uses aurutils to build AUR and local packages in a [clean chroot](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot) with the settings of `/etc/makepkg.conf`, for example the compression, `MAKEFLAGS` and debug packages. Declare this file to change them, see [makepkg](https://wiki.archlinux.org/title/Makepkg). The packages go into the local repository `/var/lib/pacnix/repo`.

The PKGBUILD of a package runs on the host as the user `pacnix`, for example to download the sources, and in the clean chroot for the build. The user `pacnix` has no sudo rule: root starts the clean chroot and adds the packages to the repository, which the user `pacnix` cannot change.

pacnix signs the local repository with a key that it makes at the first build and keeps in `/var/lib/pacnix/gnupg`. At that time, the output shows the fingerprint. To give the repository to other hosts, serve `/var/lib/pacnix/repo` with an HTTP server. On each of the other hosts, declare `packages.server` and the fingerprint in `packages.serverKey`. pacnix then downloads the public key `pacnix-local.asc` from the server, makes sure that it has the declared fingerprint, and trusts it with `pacman-key`. pacman installs only the packages that have a correct signature. To show the fingerprint again, run `gpg --show-keys /var/lib/pacnix/repo/pacnix-local.asc`.

Arch Linux does not support [partial upgrades](https://wiki.archlinux.org/title/System_maintenance#Partial_upgrades_are_unsupported). A pinned library can break the packages that need a newer version. An AUR package keeps the version in the local repository. For a different version, declare its PKGBUILD in `packages.local`.

When two declared packages conflict, pacnix refuses the plan. `pacnix upgrade` does not remove a package because of a conflict. If an upgrade needs such a removal, pacman stops, and you do the steps in the [Arch Linux news](https://archlinux.org/news/).

`apply` does not refresh the sync databases. `pacnix upgrade` runs `pacman -Syu`, builds the AUR packages again when necessary, and then applies.

Each AUR user can change an AUR package. At the first build of an AUR package, pacnix records its maintainer, its co-maintainers and its AUR dependencies in `/var/lib/pacnix`, without a question. When a later build shows a different maintainer or co-maintainer, or a new AUR dependency, pacnix asks before it changes the system. Before each new build of an AUR package, the output shows the changes of its PKGBUILD since the last build.

pacnix downloads the packages into the pacman cache before the transaction. If a mirror does not have a package version of the sync databases, the databases are old: run `pacnix upgrade`.

### Files

`files` contains files outside the home directories. Set one of these options:

- `text`: the content.
- `source`: a file that has the content.
- `target`: the target of a symbolic link.

`mode`, `user` and `group` set the permissions. pacnix writes regular files with an atomic replace. Some file systems, for example vfat, cannot keep a mode or an owner, because the mount options set them for all files. In this condition, pacnix refuses the apply before it changes the system, and the error shows the mount options. Declare the mode and the owner that the mount options give, or change the mount options in [fstab](https://wiki.archlinux.org/title/Fstab). When you remove a declaration of a file, pacnix deletes the file. If a package owns the file, pacnix restores the file of the package. pacnix does not replace a file that no package and no generation owns, unless you give `--overwrite-unmanaged`.

When possible, use the drop-in directories, for example `/etc/sysctl.d`, `/etc/systemd/system/*.d` and `/etc/default/grub.d`. Do not replace the complete file of a package.

Users and groups are files in [sysusers.d](https://man.archlinux.org/man/sysusers.d.5), and directories are files in [tmpfiles.d](https://man.archlinux.org/man/tmpfiles.d.5). sysusers.d adds users and groups. It does not change or delete them.

### Triggers

`triggers.<name>` runs `command` after a change to one of its `paths`. A path that ends with `/` includes all the files below it. Use triggers for commands such as `locale-gen`, `mkinitcpio -P` and `grub-mkconfig`. For changes that a package makes, use a [pacman hook](https://man.archlinux.org/man/alpm-hooks.5).

### Services

`units.enable`, `units.disable` and `units.mask` contain systemd system units. pacnix writes them as a [preset](https://man.archlinux.org/man/systemd.preset.5) file and runs `systemctl preset-all`. The vendor presets of the packages control the units that are not in the configuration.

### Home

`home.users.<name>` contains a [Home Manager](https://nix-community.github.io/home-manager/) module. Use it for dotfiles, dconf settings and user units. pacnix activates it after the system step. Install software with `packages`, not with `home.packages`.

### Scope

Declare the input that an application reads:

- configuration files
- policies, for example the [Firefox policies](https://mozilla.github.io/policy-templates/), also for extensions
- dconf keys
- drop-ins

Do not declare the data that an application writes, for example databases, tokens and caches. Add these paths to `claims` so that the audit does not show them. When an apply, an upgrade or a rollback removes a feature from `claims`, the question of the run shows the paths of that feature that exist. Answer `y` to delete them, or `k` to keep them: the audit then shows them with their feature. pacnix does not delete a path that a package owns, or that a declared file or a different claim includes.

pacnix does not manage these items:

- partitions and file systems
- firmware settings, for example Secure Boot and the TPM
- secrets and passwords

## Usage

Run pacnix as your user. Before it changes the system, pacnix starts itself again with `sudo`, which asks for your password one time. pacnix evaluates the configuration as the owner of the flake directory, builds packages as the user `pacnix`, and activates each home configuration as its user. Thus no file in a home directory becomes the property of root.

| Command | Function |
|---|---|
| `pacnix plan` | Shows each change that `apply` will do. It does not change the system. |
| `pacnix apply` | Makes the system agree with the configuration. |
| `pacnix upgrade` | Upgrades the system, then applies. It also shows the [Arch Linux news](https://archlinux.org/news/) since the last upgrade. |
| `pacnix rollback` | Applies the previous generation. Package versions do not go back. |
| `pacnix uninstall` | Removes pacnix, aurutils and Nix with pacman, and deletes the data, users and groups that they made. The declared files, units and packages stay. The Home Manager files become copies outside the Nix store. |
| `pacnix audit` | Shows the state that the configuration does not explain, for example files without an owner, changed package files, `.pacnew` files, packages and units that are not declared, and paths in home directories. It also shows if a reboot is necessary and the install notes of the last change. |

`apply`, `upgrade` and `rollback` ask one question at most, before they change the system, and only for items that the configuration does not show: package removals, new maintainers or new AUR dependencies of AUR packages, the Arch Linux news and the data of removed features. `--yes` accepts the items. Without a terminal, pacnix stops if there is a question and you do not give `--yes`.

At the end, `apply` and `upgrade` show the items that you must know: if a reboot is necessary (the kernel that runs is not installed, a boot image changed, or a DKMS module was built for a kernel that does not run), new or changed install notes of packages, and new `.pacnew` files.

When a download fails because of the network, for example no connection, a DNS timeout or a mirror that stops, pacnix tries again after 10 s, 30 s and 60 s, with one download at a time. Your `ParallelDownloads` setting in `/etc/pacman.conf` does not change. The next attempt uses the packages that are already in the pacman cache and in the local repository. Each other failure, for example an error in the configuration or a package that does not build, stops pacnix with the reason.

pacnix refuses to remove `base`, `pacman`, `nix`, `pacnix` and the kernel that runs, unless you give `--allow-protected-removal`. Each apply keeps a generation in the Nix profile `/nix/var/nix/profiles/pacnix`.

## See also

- [Arch Linux installation guide](https://wiki.archlinux.org/title/Installation_guide)
- [pacman](https://wiki.archlinux.org/title/Pacman) and [makepkg](https://wiki.archlinux.org/title/Makepkg)
- [systemd](https://wiki.archlinux.org/title/Systemd)
- [Nix language](https://nix.dev/manual/nix/stable/language/)
- [NixOS module system](https://nixos.org/manual/nixos/stable/#sec-writing-modules)
- [Home Manager options](https://nix-community.github.io/home-manager/options.xhtml)
