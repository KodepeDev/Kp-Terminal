//! API local para que los propios agentes orquesten kp: `kp ctl list|spawn|send|read|status|wait|close|focus`.
//! Protocolo: primera línea `TOKEN\tcmd\targ...`, el resto del stream es el cuerpo (texto del prompt).
use crate::Event;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

const USAGE: &str = "uso: kp ctl <comando>
  list                         sesiones: id, nombre, estado, carpeta
  spawn <preset> [dir] [--worktree]   lanza un agente (no roba el foco); imprime su id
  send <id> [texto...]         envía un prompt (sin texto: lo lee de stdin)
  read <id> [n]                últimas n líneas de pantalla (por defecto 50)
  status <id>                  working | blocked | idle | exited
  wait <id> [segundos]         espera a que el agente termine; imprime el estado final
  focus <id> | close <id>";

/// Arranca el servidor en 127.0.0.1:<puerto libre>. Devuelve (dirección, token).
pub fn serve(tx: Sender<Event>) -> std::io::Result<(String, String)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?.to_string();
    let token = token();
    let tok = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, tok) = (tx.clone(), tok.clone());
            std::thread::spawn(move || handle(stream, &tx, &tok));
        }
    });
    Ok((addr, token))
}

fn token() -> String {
    use std::hash::{BuildHasher, RandomState};
    // RandomState se siembra con aleatoriedad del SO.
    let a = RandomState::new().hash_one(std::process::id());
    let b = RandomState::new().hash_one(std::time::SystemTime::now());
    format!("{a:016x}{b:016x}")
}

fn handle(stream: TcpStream, tx: &Sender<Event>, tok: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut rd = BufReader::new(&stream).take(4 << 20);
    let mut head = String::new();
    let mut body = String::new();
    if rd.read_line(&mut head).is_err() || rd.read_to_string(&mut body).is_err() {
        return;
    }
    let mut parts = head.trim_end_matches(['\r', '\n']).split('\t');
    let reply = if parts.next() != Some(tok) {
        "err: token inválido".to_string()
    } else {
        let (rtx, rrx) = mpsc::channel();
        let args = parts.map(String::from).collect();
        if tx.send(Event::Ctl(args, body, rtx)).is_err() {
            return;
        }
        rrx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| "err: timeout".into())
    };
    let _ = (&stream).write_all(reply.as_bytes());
}

pub fn request(addr: &str, tok: &str, args: &[String], body: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(addr)?;
    write!(s, "{tok}\t{}\n{body}", args.join("\t"))?;
    s.shutdown(Shutdown::Write)?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

/// `kp ctl ...` ejecutado dentro de una sesión de kp. Devuelve el código de salida.
pub fn client(args: &[String]) -> i32 {
    let (Ok(addr), Ok(tok)) = (std::env::var("KP_ADDR"), std::env::var("KP_TOKEN")) else {
        eprintln!("kp ctl: ejecútalo dentro de una sesión de kp (faltan KP_ADDR/KP_TOKEN)");
        return 2;
    };
    let call = |args: &[String], body: &str| request(&addr, &tok, args, body);
    let result = match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            return 0;
        }
        Some("wait") if args.len() >= 2 => {
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3600);
            wait(&args[1], Duration::from_secs(secs), |id| call(&["status".into(), id.into()], ""))
        }
        Some("send") if args.len() >= 2 => {
            let mut body = args[2..].join(" ");
            if body.is_empty() {
                let _ = std::io::stdin().read_to_string(&mut body);
            }
            call(&args[..2], &body)
        }
        _ => call(args, ""),
    };
    match result {
        Ok(r) => {
            println!("{}", r.trim_end());
            if r.starts_with("err") { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("kp ctl: {e}");
            1
        }
    }
}

/// Espera a que el agente arranque (sale de idle) y luego a que deje de trabajar.
fn wait(id: &str, max: Duration, status: impl Fn(&str) -> std::io::Result<String>) -> std::io::Result<String> {
    let start = Instant::now();
    let mut started = false;
    loop {
        let s = status(id)?;
        let s = s.trim();
        if s.starts_with("err") || s == "exited" {
            return Ok(s.to_string());
        }
        started |= s != "idle" || start.elapsed() > Duration::from_secs(10);
        if (started && s != "working") || start.elapsed() > max {
            return Ok(s.to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
