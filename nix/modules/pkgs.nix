{ inputs, withSystem, ... }:
{
  imports = [
    inputs.pkgs-by-name-for-flake-parts.flakeModule
  ];

  perSystem =
    { config, ... }:
    {
      pkgsDirectory = ../pkgs;
      packages.default = config.packages.ghdump;
    };

  flake = {
    overlays.default =
      final: prev:
      withSystem prev.stdenv.hostPlatform.system (
        { config, ... }:
        {
          inherit (config.packages) ghdump;
        }
      );
  };
}
