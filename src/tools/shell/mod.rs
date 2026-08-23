use portable_pty::PtySize;
use rig::prelude::*;
use rig::tool::{IntoToolOutput, Tool};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use termwiz::input::{KeyCode, Modifiers};
use thiserror::Error;
use tokio::sync::RwLock;
use wezterm_term::Line;

use crate::iter::IteratorExt;
use crate::tools::Environment;
use crate::tools::shell::scrollback::Scrollback;
use crate::tools::shell::session::{Session, SessionName, SessionTermination};

mod scrollback;
mod session;
#[cfg(test)]
mod tests;

pub struct Shell {
    shell: Arc<Path>,
    env: Arc<Environment>,
    pty_size: PtySize,
    sattle_down_timeout: Duration,
    ctx: RwLock<HashMap<SessionName, (Session, Arc<RwLock<Scrollback>>)>>,
    max_scrollback_lines: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShellArgs {
    commands: Arc<[ShellCommand]>,
    /// Use this parameter to switch shell instances. Defaults to `main`.
    #[serde(default)]
    session: SessionName,
    /// Defaults to false. If set, you will be notified in a future turn.
    #[serde(default)]
    background: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShellCommand {
    /// Paste into the shell.
    Paste(String),
    /// Emulate per-keypress.
    Press(String),
    /// Send a special one.
    Send(ShellCommandSend),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShellCommandSend {
    /// Send a keypress.
    Enter,
    /// Send SIGINT.
    CtrlC,
    /// Send SIGTERM.
    Terminate,
    /// Send a keypress.
    Escape,
    /// Send a keypress.
    Tab,
}

#[derive(Debug, Error)]
pub enum ShellError {
    #[error("failed to open terminal: {0}")]
    OpenPty(anyhow::Error),
    #[error("failed to spawn shell: {0}")]
    SpawnShell(anyhow::Error),
    #[error("failed to kill process: {0}")]
    KillProcess(std::io::Error),
    #[error("write error: {0}")]
    Write(anyhow::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShellOutput {
    RunningInBackground(SessionName),
    Output(String),
}

impl Tool for Shell {
    const NAME: &'static str = "shell";

    type Args = ShellArgs;

    type Output = ShellOutput;

    type Error = ShellError;

    fn description(&self) -> String {
        if let Some(os_name) = self.env.os_name() {
            format!(
                "Access to a {} {} shell in a full-featured terminal emulator. Keypress-based operation and interactive CLI are supported. {} seconds sattle down. Only at most {} new lines returned.",
                os_name,
                self.shell.file_name().unwrap().to_string_lossy(),
                self.sattle_down_timeout.as_secs(),
                self.max_scrollback_lines,
            )
        } else {
            "Access to a shell".into()
        }
    }

    fn parameters(&self) -> serde_json::Value {
        schema_for!(ShellArgs).into()
    }

    async fn call(
        &self,
        context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let (session, scrollback) = if let Some(pack) = self.ctx.read().await.get(&args.session) {
            pack.clone()
        } else {
            let system = portable_pty::native_pty_system();
            let session = Session::new::<256, _, _>(
                args.session.clone(),
                system.as_ref(),
                self.pty_size,
                self.shell.as_os_str(),
            )?;
            let scrollback = Arc::new(RwLock::new(Scrollback::new()));
            self.ctx.write().await.insert(
                args.session.clone(),
                (session.clone(), Arc::clone(&scrollback)),
            );
            (session, scrollback)
        };
        let mut terminated = false;
        for command in args.commands.iter() {
            match command {
                ShellCommand::Paste(text) => {
                    if text.len() > 1 {
                        session.send_input(text).await?;
                    }
                }
                ShellCommand::Press(text) => {
                    let mut chars = text.chars();
                    while let Some(c) = chars.next() {
                        session.send_key(KeyCode::Char(c), Modifiers::NONE).await?;
                    }
                }

                ShellCommand::Send(ShellCommandSend::Enter) => {
                    session.send_key(KeyCode::Enter, Modifiers::NONE).await?
                }
                ShellCommand::Send(ShellCommandSend::CtrlC) => {
                    session
                        .send_key(KeyCode::Char('c'), Modifiers::CTRL)
                        .await?
                }
                ShellCommand::Send(ShellCommandSend::Terminate) => {
                    if session.send_terminate().await? == SessionTermination::Shell {
                        self.ctx.write().await.remove(&args.session);
                        terminated = true;
                        break;
                    }
                }
                ShellCommand::Send(ShellCommandSend::Escape) => {
                    session.send_key(KeyCode::Escape, Modifiers::NONE).await?;
                }
                ShellCommand::Send(ShellCommandSend::Tab) => {
                    session.send_key(KeyCode::Tab, Modifiers::NONE).await?;
                }
            }
        }
        if terminated {
            return Ok(ShellOutput::Output(
                format!(
                    "Shell process exited: SIGTERM; Session `{}` is null",
                    args.session
                )
                .into(),
            ));
        }

        if args
            .commands
            .iter()
            .any(|it| matches!(it, ShellCommand::Paste(_)))
            && !args
                .commands
                .iter()
                .any(|it| matches!(it, ShellCommand::Send(ShellCommandSend::Enter)))
        {
            session.send_key(KeyCode::Enter, Modifiers::NONE).await?;
        }

        if args.background {
            return Ok(ShellOutput::RunningInBackground(args.session));
        }

        let output = session
            .read_to_sattle_down(self.sattle_down_timeout)
            .await?;
        session.advance_bytes(output).await?;

        let all_lines = session.terminal().await.screen().all_lines();
        let unseen_lines = scrollback
            .read()
            .await
            .get_unseen(all_lines.iter())
            .into_iter()
            .last_n(self.max_scrollback_lines)
            .into();
        scrollback.write().await.update(all_lines);
        Ok(unseen_lines)
    }
}

impl Shell {
    pub fn os_default(env: Arc<Environment>) -> Self {
        Self {
            shell: PathBuf::from_str(&std::env::var("SHELL").unwrap())
                .unwrap()
                .into(),
            env,
            pty_size: PtySize::default(),
            sattle_down_timeout: Duration::from_secs(5),
            ctx: RwLock::new(HashMap::default()),
            max_scrollback_lines: 500,
        }
    }

    pub fn new(
        shell: impl Into<Arc<Path>>,
        env: impl Into<Arc<Environment>>,
        pty_size: impl Into<PtySize>,
        sattle_down_timeout: impl Into<Duration>,
        max_scrollback_lines: impl Into<usize>,
    ) -> Self {
        Self {
            shell: shell.into(),
            env: env.into(),
            pty_size: pty_size.into(),
            sattle_down_timeout: sattle_down_timeout.into(),
            ctx: RwLock::new(HashMap::default()),
            max_scrollback_lines: max_scrollback_lines.into(),
        }
    }
}

impl IntoToolOutput for ShellOutput {
    fn into_tool_output(self) -> Result<rig::tool::ToolOutput, rig::tool::ToolExecutionError> {
        match self {
            ShellOutput::RunningInBackground(session) => Ok(rig::tool::ToolOutput::text(format!(
                "Running in session `{}`",
                session
            ))),
            ShellOutput::Output(output) => Ok(rig::tool::ToolOutput::text(output)),
        }
    }
}

impl<'a, I> From<I> for ShellOutput
where
    I: IntoIterator<Item = &'a Line>,
{
    fn from(value: I) -> Self {
        Self::Output(
            value
                .into_iter()
                .map(|line| line.as_str().to_string())
                .reduce(|accu, curr| {
                    if accu.ends_with('\n') && curr.is_empty() {
                        accu
                    } else {
                        format!("{accu}\n{curr}")
                    }
                })
                .unwrap_or_default(),
        )
    }
}
