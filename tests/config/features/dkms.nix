# Kernel modules built by DKMS: one from the sync repositories and one from the AUR.
{ config, lib, ... }:
{
  options.features.dkms.enable = lib.mkEnableOption "DKMS modules";
  config = lib.mkIf config.features.dkms.enable {
    packages.repo = [ "linux-headers" "dkms" "v4l2loopback-dkms" ];
    packages.aur = [ "corsairpsu-dkms-git" ];
  };
}
