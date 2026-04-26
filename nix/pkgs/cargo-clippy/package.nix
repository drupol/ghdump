{
  lib,
  cargo,
  clippy,
  ghdump,
}:

ghdump.overrideAttrs (oldAttrs: {
  nativeCheckInputs = (oldAttrs.nativeCheckInputs or [ ]) ++ [
    cargo
    clippy
  ];

  checkPhase = ''
    RUSTFLAGS="-Dwarnings" ${lib.getExe cargo} clippy
  '';
})
