#!/usr/bin/env sh
# Тестовая сборка под Windows x64 из Linux (mingw-w64), пока CI недоступен.
# Нужны: apt install mingw-w64; rustup target add x86_64-pc-windows-gnu
# Результат: dist/Postman-test-win64/ (postman.exe, postman-gpui.exe, postman-worker.exe).
set -eu
T=x86_64-pc-windows-gnu
cargo build --release --target $T -p postman-worker -p postman
# gpui на Windows компилирует шейдеры fxc.exe из Windows SDK; в профиле xwin — при запуске.
cargo build --profile xwin --target $T -p postman-gpui
OUT=dist/Postman-test-win64
rm -rf "$OUT" && mkdir -p "$OUT"
cp target/$T/release/postman.exe target/$T/release/postman-worker.exe target/$T/xwin/postman-gpui.exe "$OUT/"
echo "Готово: $OUT"
