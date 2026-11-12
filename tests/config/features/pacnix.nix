# pacnix itself: Nix, aurutils from the AUR and pacnix from its PKGBUILD.
{ config, lib, ... }:
{
  options.features.pacnix.enable = lib.mkEnableOption "pacnix";
  config = lib.mkIf config.features.pacnix.enable {
    packages.repo = [ "nix" "devtools" "base-devel" ];
    packages.aur = [ "aurutils" ];
    packages.local.pacnix = ../src/pacnix;
    units.enable = [ "nix-daemon.socket" ];
  };
}
