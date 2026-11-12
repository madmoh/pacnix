# pacnix

pacnix manages an Arch Linux system from a declarative configuration. You write the packages, files, services and home configuration of each host in the Nix language. `pacnix apply` then makes the system agree with that configuration. pacman, makepkg and systemd make all the changes. The system stays a normal Arch Linux system.

pacnix is idempotent and it reconciles: when you remove a declaration, its package, file or service also leaves the system. When you read the configuration, you can find the cause of each item on the system.

## Installation

pacnix is in the signed pacman repository `[pacnix]`, which the releases of this project supply. Each release has the package, the repository database and the signing key `pacnix.asc`. To trust the key and add the repository, run these commands as root. During an Arch Linux installation, run them on the live system.

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

`pacstrap -K` gives the new system an empty keyring, thus the key is added again in the chroot. The apply in the chroot does these steps:

- It installs the packages from the repositories.
- It writes the files and enables the services.
- It runs the triggers, for example `locale-gen` and the installation of the boot loader.

A chroot cannot build AUR packages or activate a home configuration. On the first boot, `pacnix-firstboot.service` runs `pacnix apply` and does these two steps. To monitor it, run `journalctl -fu pacnix-firstboot`.

Set the passwords with `passwd` before you reboot. pacnix does not manage secrets.

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
  packages.aur = [ "aurutils" "visual-studio-code-bin" ];
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
| `packages.repo` | Packages from the sync repositories. |
| `packages.aur` | Packages from the [AUR](https://wiki.archlinux.org/title/Arch_User_Repository). This option needs `aurutils`. |
| `packages.local` | Directories that contain a [PKGBUILD](https://wiki.archlinux.org/title/PKGBUILD), by package name. |
| `packages.providers` | The package that satisfies a dependency, for example `{ jack = "pipewire-jack"; }`. This option has the same function as the provider choice of pacman. |
| `packages.server` | The URL of the repository of a different host. With this option, the host builds nothing. |
| `packages.serve.aur`, `packages.serve.local` | Packages that this host builds only for other hosts. |

After an apply, the explicitly installed packages are equal to the declared packages, and no orphan packages stay installed. pacnix uses [aurutils](https://github.com/aurutils/aurutils) to build AUR and local packages in a [clean chroot](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot). The packages go into the local repository `/var/lib/pacnix/repo`. To give this repository to other hosts, serve it with an HTTP server.

`apply` does not refresh the sync databases. `pacnix upgrade` runs `pacman -Syu`, builds the AUR packages again when necessary, and then applies.

### Files

`files` contains files outside the home directories. Set one of these options:

- `text`: the content.
- `source`: a file that has the content.
- `target`: the target of a symbolic link.

`mode`, `user` and `group` set the permissions. pacnix writes regular files with an atomic replace. When you remove a declaration of a file, pacnix deletes the file. If a package owns the file, pacnix restores the file of the package. pacnix does not replace a file that no package and no generation owns, unless you give `--overwrite-unmanaged`.

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

Do not declare the data that an application writes, for example databases, tokens and caches. Add these paths to `claims` so that the audit does not show them.

pacnix does not manage these items:

- partitions and file systems
- firmware settings, for example Secure Boot and the TPM
- secrets and passwords

## Usage

| Command | Function |
|---|---|
| `pacnix plan` | Shows each change that `apply` will do. It does not change the system. |
| `pacnix apply` | Makes the system agree with the configuration. Removals of packages need a confirmation, or `--yes`. |
| `pacnix upgrade` | Upgrades the system, then applies. |
| `pacnix rollback` | Applies the previous generation. Package versions do not go back. |
| `pacnix audit` | Shows the state that the configuration does not explain, for example files without an owner, changed package files, `.pacnew` files, packages and units that are not declared, and paths in home directories. |

pacnix refuses to remove `base`, `pacman`, `nix`, `pacnix` and the kernel that runs, unless you give `--allow-protected-removal`. Each apply keeps a generation in the Nix profile `/nix/var/nix/profiles/pacnix`.

## See also

- [Arch Linux installation guide](https://wiki.archlinux.org/title/Installation_guide)
- [pacman](https://wiki.archlinux.org/title/Pacman) and [makepkg](https://wiki.archlinux.org/title/Makepkg)
- [systemd](https://wiki.archlinux.org/title/Systemd)
- [Nix language](https://nix.dev/manual/nix/stable/language/)
- [NixOS module system](https://nixos.org/manual/nixos/stable/#sec-writing-modules)
- [Home Manager options](https://nix-community.github.io/home-manager/options.xhtml)
