{
  lib,
  rustPlatform,
  versionCheckHook,
}:

rustPlatform.buildRustPackage {
  pname = "ghdump";
  version = "0.2.0";

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

  cargoHash = "sha256-C/9SD2pjNEJE8Yd/v09GzgL0fLPU5pzNCAPnUZ2Zq8Q=";

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
