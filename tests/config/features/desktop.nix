# A desktop environment with its display manager. Scenario 17 switches it.
{ config, lib, ... }:
let
  desktops = {
    gnome = { packages = [ "gnome-shell" "gdm" ]; unit = "gdm.service"; };
    plasma = { packages = [ "plasma-desktop" "sddm" ]; unit = "sddm.service"; };
  };
  d = config.features.desktop;
in
{
  options.features.desktop = lib.mkOption {
    type = lib.types.nullOr (lib.types.enum (lib.attrNames desktops));
    default = null;
  };
  config = lib.mkIf (d != null) {
    packages.repo = desktops.${d}.packages;
    units.enable = [ desktops.${d}.unit ];
  };
}
