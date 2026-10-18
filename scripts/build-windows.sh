#!/usr/bin/env sh
# Тестовая сборка под Windows x64 из Linux (mingw-w64), пока CI недоступен.
# Нужны: apt install mingw-w64; rustup target add x86_64-pc-windows-gnu
# Результат: dist/Postman-test-win64/ и dist/Postman-test-win64.zip (exe, README.txt, bench.cmd, sandbox.cmd).
set -eu
T=x86_64-pc-windows-gnu
cargo build --release --target $T -p postman-worker -p postman -p postman-gpui
OUT=dist/Postman-test-win64
rm -rf "$OUT" && mkdir -p "$OUT"
cp target/$T/release/postman.exe target/$T/release/postman-worker.exe target/$T/release/postman-gpui.exe "$OUT/"
cp scripts/windows/README.txt scripts/windows/bench.cmd scripts/windows/sandbox.cmd "$OUT/"
printf '%s\r\n' "$(git rev-parse --short HEAD)$(git diff --quiet || echo '-dirty')" > "$OUT/VERSION.txt"
(cd dist && rm -f Postman-test-win64.zip && python3 -c "
import os, zipfile
with zipfile.ZipFile('Postman-test-win64.zip', 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for f in sorted(os.listdir('Postman-test-win64')):
        z.write(os.path.join('Postman-test-win64', f))
")
echo "Готово: dist/Postman-test-win64.zip"
