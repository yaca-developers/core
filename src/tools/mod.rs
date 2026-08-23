use std::sync::Arc;

use sysinfo::System;

mod shell;
mod mcp;

pub use shell::Shell;

pub struct Environment {
    os_name: Option<Arc<str>>,
    host_name: Option<Arc<str>>,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            os_name: System::name().map(|it| it.into()),
            host_name: System::host_name().map(|it| it.into()),
        }
    }
}

impl Environment {
    pub fn os_name(&self) -> Option<&str> {
        self.os_name.as_deref()
    }

    pub fn host_name(&self) -> Option<&str> {
        self.host_name.as_deref()
    }
}
