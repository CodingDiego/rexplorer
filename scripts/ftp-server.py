"""Read-only passive FTP server on 127.0.0.1 for scripts\ftp-check.ps1 (#36). Standard library only.
usage: ftp-server.py <root> <port> <user> <password> <logfile>"""
import os, socket, socketserver, sys, time

ROOT, PORT, USER, PASSWORD, LOG = os.path.abspath(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]


def log(line):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(f"{time.strftime('%H:%M:%S')} {line}\n")


class Handler(socketserver.StreamRequestHandler):
    def reply(self, text):
        self.wfile.write((text + "\r\n").encode("utf-8"))

    def local(self, arg):
        path = arg if arg.startswith("/") else self.cwd.rstrip("/") + "/" + arg
        parts = []
        for p in path.split("/"):
            if p == "..":
                parts = parts[:-1]
            elif p and p != ".":
                parts.append(p)
        return "/" + "/".join(parts), os.path.join(ROOT, *parts)

    def data(self):
        conn, _ = self.pasv.accept()
        self.pasv.close()
        self.pasv = None
        return conn

    def handle(self):
        self.cwd, self.pasv, self.rest, user, authed = "/", None, 0, "", False
        log("CONNECT")
        self.reply("220 rexplorer test server")
        while True:
            raw = self.rfile.readline()
            if not raw:
                return
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            cmd, _, arg = line.partition(" ")
            cmd = cmd.upper()
            log(f"{cmd} {'***' if cmd == 'PASS' else arg}")
            if cmd == "USER":
                user = arg
                self.reply("331 password")
            elif cmd == "PASS":
                authed = user == USER and arg == PASSWORD
                log(f"  login {'ok' if authed else 'REFUSED'} for {user}")
                self.reply("230 in" if authed else "530 no")
            elif cmd == "QUIT":
                self.reply("221 bye")
                return
            elif cmd == "FEAT":
                self.reply("211-Features\r\n SIZE\r\n MDTM\r\n PASV\r\n EPSV\r\n UTF8\r\n REST STREAM\r\n211 End")
            elif cmd in ("OPTS", "NOOP", "MODE", "STRU", "ALLO", "TYPE"):
                self.reply("200 ok")
            elif cmd == "SYST":
                self.reply("215 UNIX Type: L8")
            elif not authed:
                self.reply("530 log in")
            elif cmd in ("PWD", "XPWD"):
                self.reply(f'257 "{self.cwd}"')
            elif cmd in ("CWD", "XCWD", "CDUP"):
                virtual, real = self.local(".." if cmd == "CDUP" else arg)
                if os.path.isdir(real):
                    self.cwd = virtual
                    self.reply("250 ok")
                else:
                    self.reply("550 no such folder")
            elif cmd in ("PASV", "EPSV"):
                self.pasv = socket.socket()
                self.pasv.bind(("127.0.0.1", 0))
                self.pasv.listen(1)
                port = self.pasv.getsockname()[1]
                self.reply(f"227 Entering Passive Mode (127,0,0,1,{port >> 8},{port & 255})" if cmd == "PASV" else f"229 Entering Extended Passive Mode (|||{port}|)")
            elif cmd in ("LIST", "NLST"):
                names = [a for a in arg.split() if not a.startswith("-")]
                _, real = self.local(names[0] if names else ".")
                if not os.path.isdir(real) or self.pasv is None:
                    self.reply("550 no")
                    continue
                self.reply("150 list")
                out = []
                for n in sorted(os.listdir(real)):
                    st = os.stat(os.path.join(real, n))
                    kind = "d" if os.path.isdir(os.path.join(real, n)) else "-"
                    stamp = time.strftime("%b %d %H:%M", time.localtime(st.st_mtime))
                    out.append(n if cmd == "NLST" else f"{kind}rw-r--r-- 1 owner group {st.st_size:>10} {stamp} {n}")
                conn = self.data()
                conn.sendall(("\r\n".join(out) + "\r\n").encode("utf-8") if out else b"")
                conn.close()
                self.reply("226 done")
            elif cmd in ("SIZE", "MDTM"):
                _, real = self.local(arg)
                if os.path.isfile(real):
                    st = os.stat(real)
                    self.reply(f"213 {st.st_size}" if cmd == "SIZE" else "213 " + time.strftime("%Y%m%d%H%M%S", time.gmtime(st.st_mtime)))
                else:
                    self.reply("550 not a file")
            elif cmd == "REST":
                self.rest = int(arg or 0)
                self.reply("350 ok")
            elif cmd == "RETR":
                _, real = self.local(arg)
                if not os.path.isfile(real) or self.pasv is None:
                    self.reply("550 no")
                    continue
                self.reply("150 sending")
                conn = self.data()
                with open(real, "rb") as f:
                    f.seek(self.rest)
                    conn.sendall(f.read())
                self.rest = 0
                conn.close()
                self.reply("226 done")
            else:
                self.reply("550 read-only server" if cmd in ("STOR", "DELE", "MKD", "RMD", "RNFR", "RNTO", "APPE") else "502 not implemented")


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


log(f"listening on 127.0.0.1:{PORT}, root {ROOT}")
Server(("127.0.0.1", PORT), Handler).serve_forever()
