{
  lib,
  rustPlatform,
  versionCheckHook,
}:

rustPlatform.buildRustPackage {
  pname = "ghdump";
  version = "0.2.2";

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

  cargoHash = "sha256-gyNMtS6h2ct9IkvfhRWyMv9JVPtVEILsmYUcPETFEWQ=";

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
