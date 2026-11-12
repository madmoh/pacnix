{
  features.base.enable = true;
  features.pacnix.enable = true;
  features.zsh.enable = true;
  features.remote = { enable = true; role = "mobile"; };
  features.memory = { enable = true; swapSizeMiB = 256; };
  packages.server = "http://desktop:8080";
  packages.aur = [ "downgrade" ];
}
