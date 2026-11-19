# Scenario 12: the package lists of a typical workstation, from the most popular
# packages of pkgstats and the AUR.
{
  features.base.enable = true;
  features.pacnix.enable = true;
  features.fixtures = {
    enable = true;
    machine = "workstation";
    exclude.exfat-utils = "it conflicts with exfatprogs, which the list also has";
  };
}
