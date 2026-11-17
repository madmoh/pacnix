# Scenario 24: an encrypted Btrfs root with the subvolume @ that boots from a
# UKI, and pacnix takes over. scenario.nix gives the LUKS parameters.
{ lib, ... }:
{
  features.machine.enable = true;
  features.pacnix.enable = true;
  files."/etc/pacman.conf".source = ../files/pacman.conf;
  fileSystems."/" = lib.mkForce { device = "/dev/mapper/root"; fsType = "btrfs"; options = [ "subvol=/@" "compress=zstd" ]; };
  swapDevices = lib.mkForce [ ];
  packages.repo = [ "btrfs-progs" ];
  boot.loader.systemd-boot.uki = lib.mkDefault true;
  files."/etc/mkinitcpio.conf.d/10-encrypt.conf".text = ''
    HOOKS=(base systemd autodetect microcode modconf kms keyboard block sd-encrypt filesystems fsck)
    FILES=(/etc/cryptsetup-keys.d/root.key)
  '';
}
