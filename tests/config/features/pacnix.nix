# pacnix itself from the test repository [pacnix].
{ config, lib, ... }:
{
  options.features.pacnix.enable = lib.mkEnableOption "pacnix";
  config = lib.mkIf config.features.pacnix.enable {
    packages.repo = [ "pacnix" ];
    units.enable = [ "nix-daemon.socket" ];
  };
}
