use crate::logging;

use rig::tool::Tool;

use crate::tools::{Environment, shell::ShellOutput};

#[tokio::test]
#[test_log::test]
#[cfg(unix)]
async fn run_sh() {
    use std::{path::PathBuf, str::FromStr};

    let env = Environment {
        os_name: Some("generic unix".into()),
        host_name: Some("test-host".into()),
    };
    let mut shell = super::Shell::os_default(&env);
    shell.shell = PathBuf::from_str("/bin/sh").unwrap().into();
    let result = shell
        .call(
            &mut Default::default(),
            serde_json::from_str(r#"{"commands": [{"input": "exit"}, "enter"]}"#).unwrap(),
        )
        .await
        .expect("Run failed");
    if let ShellOutput::Output(out) = result {
        logging::info!("stdout: {}", out);
        assert!(out.contains("exit\nexit"));
    } else {
        panic!("Unexpected output: {:?}", result);
    }
}
