# Overrides of files that packages own: pacman.conf, locale.gen and a PAM file.
{ config, lib, ... }:
{
  options.features.overrides.enable = lib.mkEnableOption "overrides of package files";
  config = lib.mkIf config.features.overrides.enable {
    files."/etc/pacman.conf".text = ''
      [options]
      HoldPkg = pacman glibc
      Architecture = auto
      Color
      CheckSpace
      ParallelDownloads = 5
      DownloadUser = alpm
      SigLevel = Required DatabaseOptional
      LocalFileSigLevel = Optional

      [core]
      Include = /etc/pacman.d/mirrorlist

      [extra]
      Include = /etc/pacman.d/mirrorlist
    '';
    files."/etc/locale.gen".text = "en_US.UTF-8 UTF-8\nar_AE.UTF-8 UTF-8\n";
    triggers.locale = { paths = [ "/etc/locale.gen" ]; command = "locale-gen"; };
    files."/etc/pam.d/su".source = ../files/su;
  };
}
