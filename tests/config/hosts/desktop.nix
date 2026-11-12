# Builds the AUR repository and serves it to laptop and newpc.
{
  features.base.enable = true;
  features.pacnix.enable = true;
  features.zsh.enable = true;
  features.remote = { enable = true; role = "stationary"; };
  features.memory = { enable = true; swapSizeMiB = 512; };
  features.aur-server = { enable = true; url = "http://desktop:8080"; };
  packages.aur = [ "perl-email-outlook-message" ];
}
