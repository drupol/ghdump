{
  lib,
  rustPlatform,
  versionCheckHook,
}:

rustPlatform.buildRustPackage {
  pname = "ghdump";
  version = "0.1.2";

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

  cargoHash = "sha256-kuXtBrMk1s5mDjMVL/BKV+8qRlJ/g0Svv07IQepcQE8=";

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
