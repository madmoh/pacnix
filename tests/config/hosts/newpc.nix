{
  features.base.enable = true;
  features.pacnix.enable = true;
  features.zsh.enable = true;
  features.remote.enable = true;
  features.memory = { enable = true; swapSizeMiB = 256; };
  packages.server = "http://desktop:8080";
}
