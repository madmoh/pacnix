# Scenario 16: a machine that pacnix installs to an empty disk.
{
  features.machine.enable = true;
  features.pacnix.enable = true;
  features.home.enable = true;
  packages.local.slow = ../packages/slow;
  # The steps of the README: pacman.conf has [pacnix], and the user owns the configuration.
  files."/etc/pacman.conf".source = ../files/pacman.conf;
  files."/etc/tmpfiles.d/config.conf".text = "Z /etc/pacnix - arch arch\n";
}
