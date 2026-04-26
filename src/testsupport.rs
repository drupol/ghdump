use std::{fs, path::PathBuf};

use serde::de::DeserializeOwned;

pub(crate) fn load_json_fixture<T: DeserializeOwned>(relative_path: &str) -> T {
    let path = fixture_path(relative_path);
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"));
    serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("failed to deserialize {path:?}: {error}"))
}

fn fixture_path(relative_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(relative_path)
}
