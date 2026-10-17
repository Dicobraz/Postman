//! Процесс-воркер; запускается только клиентом `postman_worker::Client`.

fn main() {
    postman_worker::serve();
}
