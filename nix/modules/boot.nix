# Kernels, the kernel command line, the initramfs and the boot loader.
# systemd-boot gets loader.conf and, for each kernel, a unified kernel image
# (UKI) or an entry on the EFI system partition; GRUB gets a drop-in of
# /etc/default/grub and a pacman hook that writes grub.cfg after a kernel or
# initramfs change. With mkinitcpio, pacnix writes the preset of each kernel;
# the pacman hooks of mkinitcpio build the images after a kernel change, and
# the trigger initramfs runs the same hook after a change of a preset, of
# mkinitcpio.conf or of the command line, then deletes the images of the other
# format. A change of the loader configuration installs the loader when it is
# missing, and the loader registers itself in the EFI boot order. pacnix does
# not remove a boot loader. With Secure Boot, sbctl signs the boot files, and
# pacnix enrolls the keys of the host. /etc/crypttab.initramfs gives the LUKS
# devices of the initramfs, and systemd-cryptenroll enrolls their TPM2 keys.
{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  cfg = config.boot;
  esp = cfg.loader.efi.efiSysMountPoint;
  sdboot = cfg.loader.systemd-boot;
  grub = cfg.loader.grub;
  loader = sdboot.enable || grub.enable;
  uki = sdboot.enable && sdboot.uki;
  sb = sdboot.secureBoot;
  luks = cfg.initrd.luks.devices;
  root = config.fileSystems."/" or null;
  subvol = if root == null then null else lib.findFirst (o: lib.hasPrefix "subvol=" o) null root.options;
  # The kernel needs the root file system. Without a declared one, kernelParams gives it.
  params = lib.optionals (root != null) ([ "root=${root.device}" "rw" ] ++ lib.optional (subvol != null) "rootflags=${subvol}") ++ cfg.kernelParams;
  provider = config.packages.providers.initramfs or null;
  mkinitcpio = lib.elem provider [ null "mkinitcpio" ];
  initrd = if provider == "booster" then "booster" else "initramfs";
  image = k: if uki then "${esp}/EFI/Linux/arch-${k}.efi" else "/boot/${initrd}-${k}.img";
  # The images of mkinitcpio in the other format, which a change of the format leaves.
  other = k: lib.subtractLists [ (image k) ] ([ "/boot/initramfs-${k}.img" ]
    ++ lib.concatMap (d: [ "${d}/EFI/Linux/arch-${k}.efi" "${d}/EFI/Linux/arch-${k}-fallback.efi" ]) (lib.unique [ "/boot" "/efi" esp ]));
  presets = map (k: "/etc/mkinitcpio.d/${k}.preset") cfg.kernels;
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
      example = [ "quiet" "rd.luks.name=2d518b56-2cf4-47aa-a674-2d93c47542a5=root" ];
      description = "Kernel command line. With a declared root file system, pacnix adds root=, rw and the subvolume of the root before it, in /etc/kernel/cmdline, the entries and the UKIs of systemd-boot.";
    };
    loader.efi.efiSysMountPoint = mkOption {
      type = types.str;
      default = "/boot";
      example = "/efi";
      description = "Mount point of the EFI system partition. pacnix writes boot files only when a vfat file system is mounted there. systemd-boot entries without UKIs need the kernels on it, thus /boot.";
    };
    loader.systemd-boot = {
      enable = mkOption { type = types.bool; default = false; description = "systemd-boot, installed with bootctl and updated by systemd-boot-update.service."; };
      uki = mkOption {
        type = types.bool;
        default = false;
        description = "Boot each kernel from a unified kernel image in EFI/Linux/arch-KERNEL.efi on the EFI system partition, with the command line in it, instead of an entry. Needs mkinitcpio.";
      };
      secureBoot = {
        enable = mkOption {
          type = types.bool;
          default = false;
          description = "Secure Boot with the keys of this host. pacnix creates the keys with sbctl in /var/lib/sbctl, signs systemd-boot, the UKIs or kernels, the files that the loader entries on the EFI system partition boot and extraFiles, and enrolls the keys when the firmware is in Setup Mode. The pacman hooks of sbctl sign the files again after an update.";
        };
        dbAdditions = mkOption {
          type = types.listOf (types.enum [ "microsoft" "firmware-builtin" "tpm-eventlog" ]);
          default = [ "microsoft" ];
          description = "Certificates and checksums that the enrollment adds to the signature database, as db_additions in sbctl.conf(5). Keep microsoft when the firmware or a device has option ROMs that Microsoft signed.";
        };
        extraFiles = mkOption {
          type = types.listOf types.str;
          default = [ ];
          example = [ "/efi/vmlinuz-linux" ];
          description = "More EFI binaries to sign, for example a kernel of a different system.";
        };
        files = mkOption {
          type = types.listOf (types.submodule { options = { path = mkOption { type = types.str; }; output = mkOption { type = types.str; }; required = mkOption { type = types.bool; }; }; });
          internal = true;
          readOnly = true;
          description = "The boot files that pacnix signs, with the signed output.";
        };
      };
      settings = mkOption {
        type = types.attrsOf (types.oneOf [ types.str types.int types.bool ]);
        default = { };
        example = { timeout = 3; editor = false; };
        description = "Settings of loader.conf, see loader.conf(5).";
      };
    };
    initrd.luks.devices = mkOption {
      default = { };
      description = "LUKS2 devices that the initramfs opens, by mapper name, from /etc/crypttab.initramfs. Needs mkinitcpio with the hooks systemd and sd-encrypt. The passphrase of a device stays.";
      type = types.attrsOf (types.submodule {
        options = {
          device = mkOption { type = types.str; example = "UUID=2d518b56-2cf4-47aa-a674-2d93c47542a5"; description = "The device, as the second field of crypttab(5)."; };
          options = mkOption { type = types.listOf types.str; default = [ ]; example = [ "discard" ]; description = "Options of crypttab(5)."; };
          tpm2.enable = mkOption {
            type = types.bool;
            default = false;
            description = "Unlock with the TPM2 chip. pacnix enrolls a TPM2 key with systemd-cryptenroll when the firmware checks Secure Boot signatures, and enrolls it again when the values of the PCRs change, for example after new Secure Boot keys. The apply asks for the passphrase.";
          };
          tpm2.pcrs = mkOption { type = types.listOf types.int; default = [ 7 ]; description = "The PCRs that the TPM2 key needs, as --tpm2-pcrs of systemd-cryptenroll. PCR 7 has the Secure Boot state and keys."; };
          tpm2.pin = mkOption { type = types.bool; default = false; description = "Ask for a PIN at boot, as --tpm2-with-pin of systemd-cryptenroll. The apply asks for the new PIN."; };
          recoveryKey = mkOption { type = types.bool; default = false; description = "Enroll a recovery key with systemd-cryptenroll --recovery-key when the device has none. The summary shows it one time."; };
        };
      });
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
        { assertion = !loader || cfg.kernels != [ ]; message = "boot.kernels: a boot loader needs a kernel."; }
        { assertion = !uki || mkinitcpio; message = "boot.loader.systemd-boot.uki: UKIs need the initramfs provider mkinitcpio."; }
        { assertion = !sdboot.enable || uki || esp == "/boot"; message = "boot.loader.systemd-boot: entries without UKIs need the EFI system partition at /boot."; }
        { assertion = !sb.enable || sdboot.enable; message = "boot.loader.systemd-boot.secureBoot: Secure Boot needs systemd-boot."; }
        { assertion = luks == { } || mkinitcpio; message = "boot.initrd.luks.devices: the devices need the initramfs provider mkinitcpio."; }
      ];
      boot.loader.systemd-boot.secureBoot.files = lib.optionals sb.enable (
        map (f: { path = f; output = f; required = false; }) ([ "${esp}/EFI/systemd/systemd-bootx64.efi" "${esp}/EFI/BOOT/BOOTX64.EFI" ]
          ++ map (k: if uki then image k else "/boot/vmlinuz-${k}") cfg.kernels)
        ++ map (f: { path = f; output = f; required = true; }) sb.extraFiles
        ++ [ { path = "/usr/lib/systemd/boot/efi/systemd-bootx64.efi"; output = "/usr/lib/systemd/boot/efi/systemd-bootx64.efi.signed"; required = false; } ]);
    }
    (lib.mkIf sb.enable {
      packages.repo = [ "sbctl" ];
      # bootctl installs the signed systemd-boot when the .signed file exists.
      claims.secure-boot.system = [ "/var/lib/sbctl" "/usr/lib/systemd/boot/efi/systemd-bootx64.efi.signed" ];
    })
    (lib.mkIf (luks != { }) {
      packages.repo = lib.optional (lib.any (d: d.tpm2.enable) (lib.attrValues luks)) "tpm2-tss";
      files."/etc/crypttab.initramfs".text = lib.concatStrings (lib.mapAttrsToList (name: d:
        "${name} ${d.device} none ${lib.concatStringsSep "," (lib.optional d.tpm2.enable "tpm2-device=auto" ++ d.options)}\n") luks);
    })
    (lib.mkIf (params != [ ]) {
      files."/etc/kernel/cmdline".text = "${toString params}\n";
    })
    (lib.mkIf (sdboot.enable || grub.enable && grub.device == null) {
      mounts.${esp} = "vfat";
    })
    (lib.mkIf (loader && mkinitcpio) {
      files = lib.listToAttrs (map (k: lib.nameValuePair "/etc/mkinitcpio.d/${k}.preset" {
        text = ''
          ALL_kver="/boot/vmlinuz-${k}"
          PRESETS=('default')
          default_${if uki then "uki" else "image"}="${image k}"
        '';
      }) cfg.kernels);
    })
    # With another provider, the trigger deletes the images of mkinitcpio.
    (lib.mkIf loader {
      triggers.initramfs = {
        paths = presets ++ [ "/etc/mkinitcpio.conf" "/etc/mkinitcpio.conf.d/" "/etc/crypttab.initramfs" ] ++ lib.optional uki "/etc/kernel/cmdline";
        # The pacman hook of mkinitcpio also copies each kernel to ALL_kver, for example to an EFI system partition mounted after the kernel installation.
        command = lib.optionalString mkinitcpio "cd / && printf '%s\\n' usr/lib/modules/*/vmlinuz | /usr/share/libalpm/scripts/mkinitcpio install && "
          + "rm -f ${toString (lib.concatMap other cfg.kernels)}";
      };
    })
    (lib.mkIf uki { packages.repo = [ "systemd-ukify" ]; })
    (lib.mkIf sdboot.enable {
      files = {
        "${esp}/loader/loader.conf" = {
          text = lib.concatStrings (lib.mapAttrsToList (k: v: "${k} ${value v}\n")
            ({ default = if uki then "arch-${lib.head cfg.kernels}.efi" else "${lib.head cfg.kernels}.conf"; } // sdboot.settings));
          mode = null;
        };
      } // lib.optionalAttrs (!uki) (lib.listToAttrs (map (k: lib.nameValuePair "${esp}/loader/entries/${k}.conf" {
        text = ''
          title Arch Linux (${k})
          linux /vmlinuz-${k}
          initrd /${initrd}-${k}.img
          options ${toString params}
        '';
        mode = null;
      }) cfg.kernels));
      triggers.loader = {
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
      # After the trigger initramfs, which writes the images that grub.cfg lists.
      triggers.loader = { paths = [ "/etc/default/grub.d/pacnix.cfg" ] ++ presets; command = "${install} && grub-mkconfig -o /boot/grub/grub.cfg"; };
    })
  ];
}
