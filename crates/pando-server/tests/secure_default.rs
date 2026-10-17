//! A server is never open to a network by accident: without tokens or OIDC it refuses to listen
//! beyond loopback unless anonymous access is chosen explicitly.

use std::process::{Command, Stdio};
use std::time::Duration;

fn pandod() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pandod"));
    c.env_remove("PANDO_TOKENS").env_remove("PANDO_OIDC_ISSUER").env_remove("PANDO_ALLOW_ANONYMOUS");
    c.stdout(Stdio::null()).stderr(Stdio::piped());
    c
}

#[test]
fn refuses_an_anonymous_server_on_the_network() {
    let out = pandod().args(["server", "--listen", "0.0.0.0:0", "--metadata", "memory"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("refusing to serve") && err.contains("PANDO_TOKENS"), "{err}");
}

#[test]
fn serves_anonymously_on_loopback_or_when_asked() {
    for (listen, allow) in [("127.0.0.1:0", None), ("0.0.0.0:0", Some("1"))] {
        let mut c = pandod();
        c.args(["server", "--listen", listen, "--metadata", "memory"]);
        if let Some(v) = allow {
            c.env("PANDO_ALLOW_ANONYMOUS", v);
        }
        let mut child = c.spawn().unwrap();
        std::thread::sleep(Duration::from_millis(700));
        let running = child.try_wait().unwrap().is_none();
        child.kill().ok();
        assert!(running, "pandod exited on {listen} (allow={allow:?})");
    }
}
