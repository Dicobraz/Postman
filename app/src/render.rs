//! Разбор тел писем в изолированном процессе (`postman-worker`).
//! В UI-процессе HTML не разбирается никогда: при сбое воркера —
//! text/plain через тот же воркер или сообщение об ошибке.

use std::sync::mpsc;
use std::time::Duration;

use postman_cache::BodyRow;
use postman_ir::{Align, Block, Document, Inline};
use postman_worker::{Client, Level, Request};

const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Rendered {
    pub id: String,
    pub doc: Document,
    pub level: Level,
}

pub struct Renderer {
    tx: mpsc::Sender<(String, BodyRow)>,
    pub rx: mpsc::Receiver<Rendered>,
}

impl Renderer {
    pub fn spawn(repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, jobs) = mpsc::channel::<(String, BodyRow)>();
        let (done, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                let mut client = Client::new(Client::default_exe(), TIMEOUT);
                while let Ok(mut job) = jobs.recv() {
                    // При быстром листании важен только последний запрос.
                    while let Ok(newer) = jobs.try_recv() {
                        job = newer;
                    }
                    let doc = render(&mut client, &job.1);
                    if done.send(Rendered { id: job.0, doc, level: client.level() }).is_err() {
                        break;
                    }
                    repaint();
                }
            })
            .expect("поток рендера");
        Self { tx, rx }
    }

    pub fn request(&self, id: String, body: BodyRow) {
        let _ = self.tx.send((id, body));
    }
}

fn render(client: &mut Client, body: &BodyRow) -> Document {
    let html_err = match &body.html {
        Some(h) => match client.render(&Request::Html(h.clone())) {
            Ok(doc) => return doc,
            Err(e) => Some(e),
        },
        None => None,
    };
    let text_err = match &body.text {
        Some(t) => match client.render(&Request::Text(t.clone())) {
            Ok(mut doc) => {
                if let Some(e) = html_err {
                    doc.blocks.insert(0, notice(&format!("HTML-версия не показана ({e}); показан текст.")));
                }
                return doc;
            }
            Err(e) => Some(e),
        },
        None => None,
    };
    match html_err.or(text_err) {
        Some(e) => Document { blocks: vec![notice(&format!("Не удалось безопасно показать письмо: {e}"))], ..Default::default() },
        None => Document::default(),
    }
}

fn notice(text: &str) -> Block {
    Block::Paragraph { align: Align::Start, content: vec![Inline::text(format!("⚠ {text}"))] }
}
