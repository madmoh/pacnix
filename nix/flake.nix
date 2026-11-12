{
  description = "pacnix: declarative Arch Linux hosts";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    home-manager = {
      url = "github:nix-community/home-manager/release-26.05";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { nixpkgs, home-manager, ... }: {
    lib.pacnixSystem = import ./lib.nix { inherit nixpkgs home-manager; };
  };
}
