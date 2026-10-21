"""OpenSSL под Windows пишет файл в текстовом режиме: CRLF превращается в CR CR LF. Вернуть как задумано."""
import pathlib
import sys

d = pathlib.Path(sys.argv[1])
for p in d.glob("*.eml"):
    b = p.read_bytes()
    p.write_bytes(b.replace(b"\r\r\n", b"\r\n"))
    print(p.name, b.count(b"\r\r\n"))
