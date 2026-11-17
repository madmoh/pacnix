# Kernels, the kernel command line and the boot loader. systemd-boot gets
# loader.conf and one entry for each kernel on the EFI system partition; GRUB
# gets a drop-in of /etc/default/grub and a pacman hook that writes grub.cfg
# after a kernel or initramfs change. A change of the loader configuration
# installs the loader when it is missing, and the loader registers itself in
# the EFI boot order. pacnix does not remove a boot loader.
{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  cfg = config.boot;
  esp = cfg.loader.efi.efiSysMountPoint;
  sdboot = cfg.loader.systemd-boot;
  grub = cfg.loader.grub;
  root = config.fileSystems."/" or null;
  subvol = if root == null then null else lib.findFirst (o: lib.hasPrefix "subvol=" o) null root.options;
  # The kernel needs the root file system. Without a declared one, kernelParams gives it.
  params = lib.optionals (root != null) ([ "root=${root.device}" "rw" ] ++ lib.optional (subvol != null) "rootflags=${subvol}") ++ cfg.kernelParams;
  initrd = if (config.packages.providers.initramfs or null) == "booster" then "booster" else "initramfs";
  value = v: if lib.isBool v then lib.boolToString v else toString v;
  install =
    if grub.device != null then "grub-install --target=i386-pc ${grub.device}"
    else "grub-install --target=x86_64-efi --efi-directory=${esp} --bootloader-id=GRUB";
in
{
  options.boot = {
    kernels = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "linux" "linux-lts" ];
      description = "Kernel packages. pacnix installs them, and the boot loader gets an entry for each.";
    };
    kernelParams = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "quiet" "loglevel=3" ];
      description = "Kernel command line, in /etc/kernel/cmdline and the boot loader. With a declared root file system, systemd-boot also gets its root parameters.";
    };
    loader.efi.efiSysMountPoint = mkOption {
      type = types.str;
      default = "/boot";
      description = "Mount point of the EFI system partition. systemd-boot needs the kernels on it, thus /boot.";
    };
    loader.systemd-boot = {
      enable = mkOption { type = types.bool; default = false; description = "systemd-boot, installed with bootctl and updated by systemd-boot-update.service."; };
      settings = mkOption {
        type = types.attrsOf (types.oneOf [ types.str types.int types.bool ]);
        default = { };
        example = { timeout = 3; editor = false; };
        description = "Settings of loader.conf, see loader.conf(5).";
      };
    };
    loader.grub = {
      enable = mkOption { type = types.bool; default = false; description = "GRUB, installed with grub-install."; };
      device = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "/dev/sda";
        description = "Disk for GRUB on a BIOS system. null: UEFI, on the EFI system partition.";
      };
      settings = mkOption {
        type = types.attrsOf types.str;
        default = { };
        example = { GRUB_TIMEOUT = "1"; };
        description = "Variables of /etc/default/grub. When kernelParams is not empty, GRUB_CMDLINE_LINUX_DEFAULT comes from kernelParams.";
      };
    };
  };
  config = lib.mkMerge [
    {
      packages.repo = cfg.kernels;
      assertions = [
        { assertion = !(sdboot.enable && grub.enable); message = "boot.loader: enable one boot loader."; }
        { assertion = !(sdboot.enable || grub.enable) || cfg.kernels != [ ]; message = "boot.kernels: a boot loader needs a kernel."; }
      ];
    }
    (lib.mkIf (cfg.kernelParams != [ ]) {
      files."/etc/kernel/cmdline".text = "${toString cfg.kernelParams}\n";
    })
    (lib.mkIf sdboot.enable {
      files = {
        "${esp}/loader/loader.conf" = {
          text = lib.concatStrings (lib.mapAttrsToList (k: v: "${k} ${value v}\n") ({ default = "${lib.head cfg.kernels}.conf"; } // sdboot.settings));
          mode = null;
        };
      } // lib.listToAttrs (map (k: lib.nameValuePair "${esp}/loader/entries/${k}.conf" {
        text = ''
          title Arch Linux (${k})
          linux /vmlinuz-${k}
          initrd /${initrd}-${k}.img
          options ${toString params}
        '';
        mode = null;
      }) cfg.kernels);
      triggers.systemd-boot = {
        paths = [ "${esp}/loader/loader.conf" ];
        command = "bootctl --esp-path=${esp} is-installed --quiet || bootctl --esp-path=${esp} install";
      };
      units.enable = [ "systemd-boot-update.service" ];
    })
    (lib.mkIf grub.enable {
      packages.repo = [ "grub" ] ++ lib.optional (grub.device == null) "efibootmgr";
      files."/etc/default/grub.d/pacnix.cfg".text = lib.concatStrings (lib.mapAttrsToList (k: v: "${k}=\"${v}\"\n")
        (lib.optionalAttrs (cfg.kernelParams != [ ]) { GRUB_CMDLINE_LINUX_DEFAULT = toString cfg.kernelParams; } // grub.settings));
      files."/etc/pacman.d/hooks/90-pacnix-grub.hook".text = ''
        [Trigger]
        Type = Path
        Operation = Install
        Operation = Upgrade
        Operation = Remove
        Target = usr/lib/modules/*/vmlinuz
        Target = usr/lib/initcpio/*
        Target = usr/lib/booster/*
        Target = usr/lib/dracut/*
        [Action]
        Description = Writing grub.cfg...
        When = PostTransaction
        Exec = /bin/sh -c 'test ! -d /boot/grub || grub-mkconfig -o /boot/grub/grub.cfg'
      '';
      triggers.grub = { paths = [ "/etc/default/grub.d/pacnix.cfg" ]; command = "${install} && grub-mkconfig -o /boot/grub/grub.cfg"; };
    })
  ];
}
