use std::ffi::{OsStr, OsString};

#[allow(unused)]
pub struct EnvVarGuard {
    var_name: OsString,
    var_value: Option<OsString>,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(val) = self.var_value.take() {
            std::env::set_var(&self.var_name, val);
        } else {
            std::env::remove_var(&self.var_name);
        }
    }
}

#[allow(unused)]
impl EnvVarGuard {
    /// Sets the environment variable `key` to `val` and returns a guard that will reset the
    /// environment variable to its original value when dropped.
    pub fn set(key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Self {
        let var_name = OsString::from(key.as_ref());
        let var_value = std::env::var_os(&var_name);
        std::env::set_var(&var_name, val);
        Self {
            var_name,
            var_value,
        }
    }
}
