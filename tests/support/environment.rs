use konoma_test_support::{unique_tmp, TmpDir};
use std::path::PathBuf;
use std::process::Command;

pub struct Environment {
    pub root: TmpDir,
}

impl Environment {
    pub fn new(tag: &str) -> Self {
        let env = Self {
            root: unique_tmp(tag),
        };
        for name in ["home", "config", "data", "cache", "runtime", "tmp"] {
            std::fs::create_dir_all(env.path(name)).unwrap();
        }
        env
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    pub fn configure(&self, command: &mut Command) {
        for (key, name) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("TMPDIR", "tmp"),
        ] {
            command.env(key, self.path(name));
        }
        command.env_remove("KONOMA_PLUGIN_SWITCHED_FROM");
    }
}
