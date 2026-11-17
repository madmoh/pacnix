# Scenario 16: a machine that the installation guide made by hand, and pacnix takes over.
{
  features.machine.enable = true;
  features.pacnix.enable = true;
  features.home.enable = true;
  packages.local.slow = ../packages/slow;
  # The steps of the README: pacman.conf has [pacnix].
  files."/etc/pacman.conf".source = ../files/pacman.conf;
}
