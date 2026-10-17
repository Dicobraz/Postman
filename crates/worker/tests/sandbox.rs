//! Воркер в настоящей песочнице: рендер работает, побег — нет.

use std::path::PathBuf;
use std::time::Duration;

use postman_worker::{Client, Level, Probe, Request};

fn client() -> Client {
    Client::new(PathBuf::from(env!("CARGO_BIN_EXE_postman-worker")), Duration::from_secs(3))
}

/// Побег не удался: либо ОС вернула ошибку, либо seccomp убил процесс.
fn denied(r: Result<postman_ir::Document, String>) {
    let e = r.expect_err("должно быть отказано");
    assert!(!e.contains("ДОСТУП РАЗРЕШЁН"), "{e}");
}

#[test]
fn renders_and_survives_escape_attempts() {
    let mut c = client();
    let doc = c.render(&Request::Html("<p><b>Привет</b><script>x</script></p>".into())).unwrap();
    assert_eq!(postman_ir::to_markdown(&doc), "**Привет**");
    assert_eq!(c.level(), Level::Full);

    let probes = if cfg!(windows) {
        vec![
            Probe::ReadFile(r"C:\Windows\win.ini".into()),
            Probe::Connect("1.1.1.1:443".into()),
            Probe::Spawn("cmd.exe".into()),
        ]
    } else {
        vec![
            Probe::ReadFile("/etc/hostname".into()),
            Probe::Connect("1.1.1.1:443".into()),
            Probe::Spawn("/bin/sh".into()),
        ]
    };
    for p in probes {
        denied(c.render(&Request::Probe(p)));
        // После убитого процесса клиент поднимает новый.
        let doc = c.render(&Request::Text("ещё жив".into())).unwrap();
        assert_eq!(postman_ir::to_markdown(&doc), "ещё жив");
    }
}

#[test]
fn hang_is_killed_by_timeout() {
    let mut c = client();
    let e = c.render(&Request::Probe(Probe::Hang)).unwrap_err();
    assert!(e.contains("не ответил"), "{e}");
    assert!(c.render(&Request::Text("ok".into())).is_ok());
}

#[test]
fn hostile_input() {
    let mut c = client();
    let deep = "<div><span>".repeat(100_000) + "x";
    let r = c.render(&Request::Html(deep));
    assert!(r.is_ok(), "{:?}", r.err());
    let wide = "<p>a</p>".repeat(30_000);
    assert!(c.render(&Request::Html(wide)).is_ok());
}
