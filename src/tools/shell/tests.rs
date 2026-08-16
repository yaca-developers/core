#[cfg(unix)]
use std::sync::Arc;

use crate::logging;
#[cfg(unix)]
use crate::tools::shell::{BySessionNameContext, session::SessionName};

use rig::tool::Tool;
#[cfg(unix)]
use rig::tool::ToolContext;

use crate::tools::{Environment, shell::ShellOutput};

/// Shared test fixture: a generic unix environment description.
#[cfg(unix)]
fn unix_env() -> Arc<Environment> {
    Environment {
        os_name: Some("generic unix".into()),
        host_name: Some("test-host".into()),
    }
    .into()
}

#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn run_sh() {
    use std::{path::PathBuf, str::FromStr};

    let env = unix_env();
    let mut shell = super::Shell::os_default(env);
    shell.shell = PathBuf::from_str("/bin/sh").unwrap().into();

    assert!(
        shell.parameters().to_string().contains("Defaults to false"),
        "schemar does not contain documentation"
    );

    let mut tool_context = ToolContext::default();
    let result = shell
        .call(
            &mut tool_context,
            serde_json::from_str(r#"{"commands": [{"input": "for i in $(seq 1 1000); do echo $i; done"}, "enter", {"input": "exit"}, "enter"]}"#).unwrap(),
        )
        .await
        .expect("Run failed");
    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        for i in 1..=1000 {
            assert!(out.contains(&format!("{}\n", i)), "missing {i}");
        }

        if let Some(sessions) = tool_context.get::<BySessionNameContext>() {
            let read_mutex = sessions.read().await;
            let (main_session, _) = read_mutex
                .get(&SessionName::from("main".to_string()))
                .unwrap();
            assert!(!main_session.is_alive().await)
        }
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// Shared test fixture: a `Shell` wired up to `/bin/sh` for the given environment.
#[cfg(unix)]
fn sh_shell(env: Arc<Environment>) -> super::Shell {
    use std::{path::PathBuf, str::FromStr};

    let mut shell = super::Shell::os_default(env);
    shell.shell = PathBuf::from_str("/bin/sh").unwrap().into();
    shell
}

#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn echo_hello_is_reflected_in_output() {
    let env = unix_env();
    let mut shell = sh_shell(env);

    let result = shell
        .call(
            &mut Default::default(),
            serde_json::from_str(r#"{"commands": [{"input": "echo hello_world"}, "enter"]}"#)
                .unwrap(),
        )
        .await
        .expect("Run failed");

    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        assert!(out.contains("hello_world"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// Reusing the same session name across calls (with the same `ToolContext`) should
/// reuse the same underlying shell process, so exported state should persist.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn session_state_persists_across_calls() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": [{"input": "FOO=bar123; export FOO"}, "enter"]}"#)
                .unwrap(),
        )
        .await
        .expect("First call failed");

    let result = shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": [{"input": "echo $FOO"}, "enter"]}"#).unwrap(),
        )
        .await
        .expect("Second call failed");

    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        assert!(out.contains("bar123"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// Two distinct session names should map to two distinct shell processes with
/// independent state.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn distinct_sessions_do_not_share_state() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    shell
        .call(
            &mut context,
            serde_json::from_str(
                r#"{"commands": [{"input": "FOO=only_in_a; export FOO"}, "enter"], "session": "session_a"}"#,
            )
            .unwrap(),
        )
        .await
        .expect("session_a setup failed");

    let result = shell
        .call(
            &mut context,
            serde_json::from_str(
                r#"{"commands": [{"input": "echo [$FOO]"}, "enter"], "session": "session_b"}"#,
            )
            .unwrap(),
        )
        .await
        .expect("session_b call failed");

    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        assert!(!out.contains("only_in_a"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// `terminate` should kill the shell process and remove the session, so a later
/// call using the same session name spawns a brand new shell.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn terminate_kills_shell_and_allows_fresh_restart() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    let result = shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": ["terminate"]}"#).unwrap(),
        )
        .await
        .expect("Terminate failed");

    match result {
        ShellOutput::Output(out) => assert!(out.contains("Session `main` is null")),
        other => panic!("Unexpected output: {:?}", other),
    }

    let result = shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": [{"input": "echo restarted"}, "enter"]}"#)
                .unwrap(),
        )
        .await
        .expect("Restart failed");

    if let ShellOutput::Output(out) = result {
        assert!(out.contains("restarted"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// `background: true` should return without waiting for the command to settle.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn background_flag_returns_immediately_with_session_name() {
    let env = unix_env();
    let mut shell = sh_shell(env);

    let start = std::time::Instant::now();
    let result = shell
        .call(
            &mut Default::default(),
            serde_json::from_str(
                r#"{"commands": [{"input": "sleep 5"}, "enter"], "background": true}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Background call failed");

    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "background call should not wait for the command to finish"
    );

    match result {
        ShellOutput::RunningInBackground(session) => assert_eq!(session.to_string(), "main"),
        other => panic!("Expected RunningInBackground, got {:?}", other),
    }
}

/// Sending `ctrl_c` should interrupt a long-running foreground command and
/// return control of the shell.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn ctrl_c_interrupts_long_running_command() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    shell
        .call(
            &mut context,
            serde_json::from_str(
                r#"{"commands": [{"input": "sleep 30"}, "enter"], "background": true}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Failed to start sleep");

    let result = shell
        .call(
            &mut context,
            serde_json::from_str(
                r#"{"commands": ["ctrl_c", {"input": "echo back_in_control"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Ctrl-C call failed");

    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        assert!(out.contains("back_in_control"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// A single-character `Input` should be delivered as a keypress rather than a
/// pasted string, e.g. answering an interactive prompt.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn single_character_input_is_sent_as_keypress() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": [{"input": "read -r answer"}, "enter"]}"#)
                .unwrap(),
        )
        .await
        .expect("Failed to start read");

    let result = shell
        .call(
            &mut context,
            serde_json::from_str(
                r#"{"commands": [{"input": "y"}, "enter", {"input": "echo got:$answer"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Failed to answer prompt");

    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        assert!(out.contains("got:y"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

/// Multiple commands in a single call should be executed in order.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn multiple_commands_execute_in_sequence() {
    let env = unix_env();
    let mut shell = sh_shell(env);

    let result = shell
        .call(
            &mut Default::default(),
            serde_json::from_str(
                r#"{"commands": [{"input": "cd /tmp"}, "enter", {"input": "pwd"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Run failed");

    if let ShellOutput::Output(out) = result {
        assert!(out.contains("/tmp"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}

#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn empty_command_list_is_a_no_op() {
    let env = unix_env();
    let mut shell = sh_shell(env);

    let result = shell
        .call(
            &mut Default::default(),
            serde_json::from_str(r#"{"commands": []}"#).unwrap(),
        )
        .await;

    assert!(result.is_ok());
}

/// `escape` and `tab` don't have deterministic visible output on a plain `sh`
/// prompt, but they should be handled without erroring.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn escape_and_tab_do_not_error() {
    let env = unix_env();
    let mut shell = sh_shell(env);

    let result = shell
        .call(
            &mut Default::default(),
            serde_json::from_str(
                r#"{"commands": [{"input": "ech"}, "tab", "escape", {"input": "o"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await;

    assert!(result.is_ok());
}

#[test]
fn shell_args_apply_defaults_when_omitted() {
    let args: super::ShellArgs =
        serde_json::from_str(r#"{"commands": [{"input": "ls"}]}"#).unwrap();

    assert_eq!(args.commands.len(), 1);
    assert_eq!(args.session.to_string(), "main");
    assert!(!args.background);
}

#[test]
fn shell_command_rejects_unknown_variant() {
    let result: Result<super::ShellArgs, _> =
        serde_json::from_str(r#"{"commands": [{"not_a_real_command": "x"}]}"#);

    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn description_includes_os_name_and_shell_name() {
    let env = unix_env();
    let shell = sh_shell(env);

    assert_eq!(shell.description(), "Access to a generic unix sh shell.");
}

#[test]
#[cfg(unix)]
fn description_falls_back_without_os_name() {
    use std::{path::PathBuf, str::FromStr};

    let env = Environment {
        os_name: None,
        host_name: None,
    };
    let mut shell = super::Shell::os_default(env.into());
    shell.shell = PathBuf::from_str("/bin/sh").unwrap().into();

    assert_eq!(shell.description(), "Access to a shell");
}

/// Drives an interactive `vim` session: open a file, enter insert mode,
/// type text, then save and quit with `:wq`. Exercises `escape` plus
/// mixed single-char (keypress) and multi-char (paste) `input` handling.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn vim_edit_and_save_file() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    let file_path = format!(
        "/tmp/shell_vim_test_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = std::fs::remove_file(&file_path); // start clean, just in case

    // Open the file in vim.
    let opened = shell
        .call(
            &mut context,
            serde_json::from_str(&format!(
                r#"{{"commands": [{{"input": "nvim {}"}}, "enter"]}}"#,
                file_path
            ))
            .unwrap(),
        )
        .await
        .expect("Failed to open vim");
    if let ShellOutput::Output(out) = &opened {
        logging::info!("vim opened:\n{}", out);
    }

    // Enter insert mode ('i' is a single char, so it's sent as a keypress)
    // and paste some text.
    shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": [{"input": "i"}, {"input": "hello from vim"}]}"#)
                .unwrap(),
        )
        .await
        .expect("Failed to enter insert mode and type text");

    // Leave insert mode and write + quit.
    let saved_screen = shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": ["escape", {"input": ":wq"}, "enter"]}"#).unwrap(),
        )
        .await
        .expect("Failed to save and quit vim");
    if let ShellOutput::Output(out) = &saved_screen {
        logging::info!("after :wq:\n{}", out);
    }

    let saved = std::fs::read_to_string(&file_path)
        .unwrap_or_else(|e| panic!("Expected {} to exist after :wq: {}", file_path, e));
    let _ = std::fs::remove_file(&file_path);

    assert!(saved.contains("hello from vim"));
}

/// Companion test: quitting with `:q!` after making changes should discard
/// them, leaving the on-disk file untouched.
#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn vim_quit_without_saving_discards_changes() {
    let env = unix_env();
    let mut shell = sh_shell(env);
    let mut context = Default::default();

    let file_path = format!(
        "/tmp/shell_vim_discard_test_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::fs::write(&file_path, "original content\n").expect("Failed to seed file");

    shell
        .call(
            &mut context,
            serde_json::from_str(&format!(
                r#"{{"commands": [{{"input": "vim {}"}}, "enter"]}}"#,
                file_path
            ))
            .unwrap(),
        )
        .await
        .expect("Failed to open vim");

    shell
        .call(
            &mut context,
            serde_json::from_str(
                r#"{"commands": [{"input": "i"}, {"input": "this should not be saved"}]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Failed to type into vim");

    shell
        .call(
            &mut context,
            serde_json::from_str(r#"{"commands": ["escape", {"input": ":q!"}, "enter"]}"#).unwrap(),
        )
        .await
        .expect("Failed to quit vim without saving");

    let contents = std::fs::read_to_string(&file_path).expect("File should still exist");
    let _ = std::fs::remove_file(&file_path);

    assert_eq!(contents, "original content\n");
    assert!(!contents.contains("this should not be saved"));
}

#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn only_unseen_lines_are_shown() {
    use std::{path::PathBuf, str::FromStr};

    let env = unix_env();
    let mut shell = super::Shell::os_default(env);
    shell.shell = PathBuf::from_str("/bin/sh").unwrap().into();
    let mut tool_context = ToolContext::default();
    let result = shell
        .call(
            &mut tool_context,
            serde_json::from_str(
                r#"{"commands": [{"input": "for i in $(seq 1 10); do echo $i; done"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Run failed");
    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        for i in 1..=10 {
            assert!(out.contains(&format!("{}\n", i)), "missing {i}");
        }
    } else {
        panic!("Unexpected output: {:?}", result);
    }

    let result = shell
        .call(
            &mut tool_context,
            serde_json::from_str(
                r#"{"commands": [{"input": "for i in $(seq 1 10); do echo $i; done"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Run failed");
    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        for i in 1..=10 {
            assert!(out.contains(&format!("{}\n", i)), "missing {i}");
        }
    } else {
        panic!("Unexpected output: {:?}", result);
    }

    let result = shell
        .call(
            &mut tool_context,
            serde_json::from_str(
                r#"{"commands": [{"input": "for i in $(seq 11 20); do echo $i; done"}, "enter"]}"#,
            )
            .unwrap(),
        )
        .await
        .expect("Run failed");
    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        for i in 11..=20 {
            assert!(out.contains(&format!("{}\n", i)), "missing {i}");
        }
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}
