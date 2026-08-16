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
    Input(String),
    /// Send a keypress. Should be sent to run the command.
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

type BySessionNameContext = Arc<RwLock<HashMap<SessionName, (Session, Arc<RwLock<Scrollback>>)>>>;

impl Tool for Shell {
    const NAME: &'static str = "shell";

    type Args = ShellArgs;

    type Output = ShellOutput;

    type Error = ShellError;

    fn description(&self) -> String {
        if let Some(os_name) = self.env.os_name() {
            format!(
                "Access to a {} {} shell.",
                os_name,
                self.shell.file_name().unwrap().to_string_lossy()
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
        let (session, scrollback) = if let Some(session_by_name) =
            context.get::<BySessionNameContext>()
            && let Some(pack) = session_by_name.read().await.get(&args.session)
        {
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
            context.insert(BySessionNameContext::new(RwLock::new(
                [(
                    args.session.clone(),
                    (session.clone(), Arc::clone(&scrollback)),
                )]
                .into(),
            )));
            (session, scrollback)
        };
        let mut terminated = false;
        for command in args.commands.iter() {
            match command {
                ShellCommand::Input(text) => {
                    if text.len() > 1 {
                        session.send_input(text).await?;
                    } else if !text.is_empty() {
                        session
                            .send_key(KeyCode::Char(text.chars().next().unwrap()), Modifiers::NONE)
                            .await?;
                    }
                }

                ShellCommand::Enter => session.send_key(KeyCode::Enter, Modifiers::NONE).await?,
                ShellCommand::CtrlC => {
                    session
                        .send_key(KeyCode::Char('c'), Modifiers::CTRL)
                        .await?
                }
                ShellCommand::Terminate => {
                    if session.send_terminate().await? == SessionTermination::Shell {
                        if let Some(session_map) = context.get::<BySessionNameContext>() {
                            session_map.write().await.remove(&args.session);
                        }
                        terminated = true;
                        break;
                    }
                }
                ShellCommand::Escape => {
                    session.send_key(KeyCode::Escape, Modifiers::NONE).await?;
                }
                ShellCommand::Tab => {
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
            .any(|it| matches!(it, ShellCommand::Input(_)))
            && !args
                .commands
                .iter()
                .any(|it| matches!(it, ShellCommand::Enter))
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
        let unseen_lines = scrollback.read().await.get_unseen(all_lines.iter()).into();
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
