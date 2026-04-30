{
  lib,
  rustPlatform,
  versionCheckHook,
}:

rustPlatform.buildRustPackage {
  pname = "ghdump";
  version = "0.1.1";

  __structuredAttrs = true;

  src = lib.fileset.toSource {
    root = ../../..;
    fileset = lib.fileset.unions [
      ../../../Cargo.toml
      ../../../Cargo.lock
      ../../../fixtures
      ../../../src
      ../../../templates
    ];
  };

  cargoHash = "sha256-G24vq74B8NyRq++FlyvR946MWKE0x1n1phTfxFSF8Gs=";

  dontUseCargoParallelTests = true;

  doInstallCheck = true;
  nativeInstallCheckInputs = [ versionCheckHook ];

  meta = {
    description = "Command-line tool to export GitHub issues, pull requests, and discussions";
    homepage = "https://github.com/drupol/ghdump";
    license = lib.licenses.eupl12;
    mainProgram = "ghdump";
    maintainers = with lib.maintainers; [ drupol ];
    platforms = lib.platforms.all;
  };
}
