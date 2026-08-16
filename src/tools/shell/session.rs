use portable_pty::{CommandBuilder, PtyPair, PtySize, PtySystem};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::ffi::OsStr;
use std::fmt::Display;
use std::io::{Read, Write};
use std::time::Duration;
use std::usize;
use std::{borrow::Cow, ops::Deref, sync::Arc};
use termwiz::escape::parser::Parser;
use termwiz::input::{KeyCode, Modifiers};
use tokio::select;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, RwLock, RwLockReadGuard, broadcast, mpsc};
use wezterm_term::performer::Performer;
use wezterm_term::{TerminalConfiguration, TerminalSize, TerminalState};

use crate::logging;
use crate::tools::shell::ShellError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct SessionName(SmolStr);

#[derive(Clone)]
pub struct Session {
    name: SessionName,
    pty_pair: Arc<Mutex<PtyPair>>,
    shell_process: Arc<RwLock<Box<dyn portable_pty::Child + Send + Sync>>>,
    read_receiver: Arc<Mutex<mpsc::Receiver<Arc<[u8]>>>>,
    terminal: Arc<RwLock<TerminalState>>,
    read_task: Arc<std::thread::JoinHandle<Result<(), std::io::Error>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionTermination {
    Shell,
    Subprocess,
}

impl Session {
    pub fn new<const BUFFER_SIZE: usize, S, I>(
        name: SessionName,
        system: S,
        pty_size: PtySize,
        command: impl AsRef<OsStr>,
    ) -> Result<Session, ShellError>
    where
        I: PtySystem + ?Sized,
        S: Deref<Target = I>,
    {
        let pty = system
            .openpty(pty_size.clone())
            .map_err(ShellError::OpenPty)?;
        let cmd = CommandBuilder::new(command);
        let child = pty
            .slave
            .spawn_command(cmd)
            .map_err(ShellError::SpawnShell)?;
        let writer = pty.master.take_writer().map_err(ShellError::SpawnShell)?;
        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(ShellError::SpawnShell)?;
        let (read_sender, read_receiver) = mpsc::channel(4);

        let mut terminal_size = TerminalSize::default();
        terminal_size.rows = pty_size.rows as usize;
        terminal_size.cols = pty_size.cols as usize;
        terminal_size.pixel_width = pty_size.pixel_width as usize;
        terminal_size.pixel_height = pty_size.pixel_height as usize;

        Ok(Session {
            name,
            pty_pair: Arc::new(Mutex::new(pty)),
            shell_process: Arc::new(RwLock::new(child)),
            terminal: Arc::new(RwLock::new(TerminalState::new(
                terminal_size,
                Arc::new(TermConfig),
                "wezterm".into(),
                "0.1.0".into(),
                writer,
            ))),
            read_receiver: Arc::new(Mutex::new(read_receiver)),
            read_task: Arc::new(std::thread::spawn(move || {
                let mut buf = [0u8; BUFFER_SIZE];
                let mut bytes_sent = 0;
                loop {
                    match reader.read(&mut buf) {
                        Ok(bytes_read) => {
                            if bytes_read == 0 {
                                break;
                            }
                            bytes_sent += bytes_read;
                            if let Err(err) = read_sender.blocking_send(buf[..bytes_read].into()) {
                                logging::error!("failed to send terminal data: {err}");
                            }
                        }
                        Err(err) => match err.kind() {
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => {}
                            _ => return Err(err),
                        },
                    }
                }
                logging::debug!("sent {} bytes", bytes_sent);
                Ok(())
            })),
        })
    }

    pub async fn send_key(&self, key: KeyCode, mods: Modifiers) -> Result<(), ShellError> {
        let mut terminal = self.terminal.write().await;
        terminal.key_down(key, mods).map_err(ShellError::Write)?;
        terminal.key_up(key, mods).map_err(ShellError::Write)?;
        Ok(())
    }

    pub async fn send_input(&self, text: impl AsRef<str>) -> Result<(), ShellError> {
        self.terminal
            .write()
            .await
            .send_paste(text.as_ref())
            .map_err(ShellError::Write)
    }

    pub async fn send_terminate(&self) -> Result<SessionTermination, ShellError> {
        // TODO: terminate child process
        self.shell_process
            .write()
            .await
            .kill()
            .map_err(ShellError::KillProcess)?;
        Ok(SessionTermination::Shell)
    }

    pub async fn read_to_sattle_down(&self, timeout: Duration) -> Result<Box<[u8]>, ShellError> {
        let mut receiver = self.read_receiver.lock().await;
        let mut accumulator = Vec::new();
        let mut bytes_read = 0;
        loop {
            select! {
                recv = receiver.recv() => {
                    let Some(bytes) = recv else {
                        break;
                    };
                    accumulator.extend_from_slice(&bytes);
                    bytes_read += bytes.len();
                }
                _ = tokio::time::sleep(timeout) => {
                    break
                }
            }
        }
        logging::debug!("read {} bytes", bytes_read);
        return Ok(accumulator.into_boxed_slice());
    }

    pub async fn advance_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<(), ShellError> {
        let mut terminal = self.terminal.write().await;
        terminal.increment_seqno();
        {
            let mut performer = Performer::new(&mut terminal);
            let mut parser = Parser::new();
            parser.parse(bytes.as_ref(), |action| performer.perform(action));
        }
        terminal.trigger_unseen_output_notif();
        Ok(())
    }

    pub async fn terminal(&self) -> RwLockReadGuard<'_, TerminalState> {
        self.terminal.read().await
    }

    pub async fn is_alive(&self) -> bool {
        self.shell_process
            .write()
            .await
            .try_wait()
            .is_ok_and(|it| it.is_none())
    }
}

#[derive(Debug)]
struct TermConfig;

impl TerminalConfiguration for TermConfig {
    fn color_palette(&self) -> wezterm_term::color::ColorPalette {
        Default::default()
    }
}

impl From<String> for SessionName {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl Into<String> for SessionName {
    fn into(self) -> String {
        self.0.to_string()
    }
}

impl Deref for SessionName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl JsonSchema for SessionName {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        Cow::Borrowed("session_name")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as JsonSchema>::json_schema(generator)
    }
}

impl Default for SessionName {
    fn default() -> Self {
        Self(SmolStr::new_static("main"))
    }
}

impl Display for SessionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
