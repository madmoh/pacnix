# Scenario 26: an encrypted root and systemd-boot with a UKI, on a firmware with
# Secure Boot and a TPM. scenario.nix declares Secure Boot and the LUKS device.
{ lib, ... }:
{
  features.machine.enable = true;
  features.pacnix.enable = true;
  files."/etc/pacman.conf".source = ../files/pacman.conf;
  fileSystems."/" = lib.mkForce { device = "/dev/mapper/root"; fsType = "ext4"; options = [ "rw" "relatime" ]; };
  swapDevices = lib.mkForce [ ];
  boot.loader.systemd-boot.uki = true;
  # The prompts of the initramfs go to the serial console, where the test types.
  boot.kernelParams = [ "console=tty0" "console=ttyS0" ];
  files."/etc/mkinitcpio.conf.d/10-encrypt.conf".text = ''
    HOOKS=(base systemd autodetect microcode modconf kms keyboard block sd-encrypt filesystems fsck)
  '';
}
