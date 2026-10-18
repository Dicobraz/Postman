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
    // Файл пользователя: AppContainer и Landlock не должны давать его прочитать.
    let secret = std::env::temp_dir().join(format!("postman-secret-{}.txt", std::process::id()));
    std::fs::write(&secret, "секрет").unwrap();
    let secret_path = secret.to_string_lossy().into_owned();

    // На Windows полная изоляция (AppContainer) требует права на чтение exe для
    // ALL APPLICATION PACKAGES — так в Program Files и в CI (см. .github/workflows/ci.yml).
    // В остальных случаях воркер работает без AppContainer: файлы и сеть ему доступны,
    // проверяем только то, что даёт Job Object и mitigation policies.
    let full = c.level() == Level::Full;
    if cfg!(windows) && std::env::var_os("POSTMAN_REQUIRE_FULL_SANDBOX").is_none() {
        assert!(matches!(c.level(), Level::Full | Level::Partial));
    } else {
        assert_eq!(c.level(), Level::Full);
    }
    let shell = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" };
    let mut probes = vec![Probe::Spawn(shell.into())];
    if full {
        probes.push(Probe::ReadFile(secret_path));
        probes.push(Probe::Connect("1.1.1.1:443".into()));
    } else {
        eprintln!("песочница частичная: проверки чтения файлов и сети пропущены");
    }
    for p in probes {
        denied(c.render(&Request::Probe(p)));
        // После убитого процесса клиент поднимает новый.
        let doc = c.render(&Request::Text("ещё жив".into())).unwrap();
        assert_eq!(postman_ir::to_markdown(&doc), "ещё жив");
    }
    let _ = std::fs::remove_file(&secret);
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

#[test]
fn decodes_images_and_rejects_others() {
    let mut c = client();
    // PNG 1x1, красный пиксель.
    let png = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49,
        0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let img = c.decode_image(&png, 512).unwrap();
    assert_eq!((img.width, img.height), (1, 1));
    assert_eq!(&img.pixels[..3], &[255, 0, 0]);
    assert!(c.decode_image(b"<svg onload=alert(1)>", 512).is_err());
    assert!(c.decode_image(&png[..20], 512).is_err());
    // Воркер жив после ошибок разбора.
    assert!(c.render(&Request::Text("ok".into())).is_ok());
}

#[test]
fn all_supported_formats_decode_under_seccomp() {
    let mut c = client();
    for ext in ["jpg", "gif", "webp", "png"] {
        let bytes = std::fs::read(format!("{}/tests/fixtures/img.{ext}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let img = c.decode_image(&bytes, 100).unwrap_or_else(|e| panic!("{ext}: {e}"));
        assert_eq!((img.width, img.height), (100, 67), "{ext}");
    }
}

#[test]
fn parses_invitation_under_seccomp() {
    let mut c = client();
    let ics = std::fs::read(format!("{}/tests/fixtures/invite-outlook.ics", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let v = c.parse_calendar(&ics).unwrap();
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    assert_eq!(v["method"], "REQUEST");
    assert!(c.parse_calendar(b"not a calendar").is_err());
}
