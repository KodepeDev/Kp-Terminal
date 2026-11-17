//! E2E: ejecuta el binario kp dentro de un PTY, lo maneja con teclas y lee su pantalla.
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Ctrl+Space y Esc tal como los envía Windows Terminal (win32-input-mode): ConPTY descarta
/// un NUL crudo y retiene un ESC suelto.
const PREFIX: &[u8] = if cfg!(windows) { b"\x1b[32;57;0;1;8;1_" } else { b"\x00" };
const ESC: &[u8] = if cfg!(windows) { b"\x1b[27;1;27;1;0;1_" } else { b"\x1b" };

struct Kp {
    child: Box<dyn Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    parser: Arc<Mutex<vt100::Parser>>,
}

impl Kp {
    fn start(dir: &std::path::Path) -> Kp {
        Kp::launch(dir, 40, 140, "")
    }

    /// kp en un PTY de `rows`x`cols` con su propia config (aislada de la del usuario y de otros tests).
    fn launch(dir: &std::path::Path, rows: u16, cols: u16, config: &str) -> Kp {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let pty = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_kp"));
        cmd.arg(dir);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let cfgdir = std::env::temp_dir().join(format!("kp-e2e-cfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(cfgdir.join("kodepeter")).unwrap();
        std::fs::write(cfgdir.join("kodepeter").join("config.toml"), config).unwrap();
        cmd.env("APPDATA", &cfgdir);
        cmd.env("XDG_CONFIG_HOME", &cfgdir);
        cmd.env_remove("NO_COLOR"); // la sesión que ejecuta los tests puede tenerla definida
        let child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows.try_into().unwrap(), cols.try_into().unwrap(), 0)));
        let (p, w) = (parser.clone(), writer.clone());
        std::thread::spawn(move || {
            let _master = pty.master; // mantener vivo el PTY
            let mut buf = [0u8; 8192];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                // ConPTY pregunta la posición del cursor al arrancar y espera respuesta.
                if buf[..n].windows(4).any(|x| x == b"\x1b[6n") {
                    let _ = w.lock().unwrap().write_all(b"\x1b[1;1R");
                }
                p.lock().unwrap().process(&buf[..n]);
            }
        });
        Kp { child, writer, parser }
    }

    fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    fn send(&self, b: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(b).unwrap();
        w.flush().unwrap();
        drop(w);
        std::thread::sleep(Duration::from_millis(200));
    }

    fn wait_for(&self, secs: u64, label: &str, what: impl Fn(&str) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if what(&self.screen()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("no apareció {label}. Pantalla:\n{}", self.screen());
    }

    fn quit(mut self) {
        self.send(PREFIX);
        self.send(b"q");
        self.send(PREFIX); // segunda vez por si pide confirmación
        self.send(b"q");
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("kp no salió con Ctrl+Space q. Pantalla:\n{}", self.screen());
    }
}

#[test]
fn drives_tui() {
    let kp = Kp::start(&std::env::temp_dir());
    kp.wait_for(20, "pestaña con shell", |s| s.contains("1 shell"));

    // Paleta (Ctrl+Space p) y salir con Esc.
    kp.send(PREFIX);
    kp.send(b"p");
    kp.wait_for(20, "paleta", |s| s.contains("paleta") && s.contains("nuevo claude"));
    kp.send(ESC);
    kp.wait_for(20, "cierre de la paleta", |s| !s.contains("nuevo claude"));

    // Segundo shell (Ctrl+Space n).
    kp.send(PREFIX);
    kp.send(b"n");
    kp.wait_for(20, "segunda sesión", |s| s.contains("2 shell"));

    // Escribir en el shell enfocado y usar la API ctl desde dentro.
    std::thread::sleep(Duration::from_secs(2));
    kp.send(if cfg!(windows) { b"& $env:KP_BIN ctl list\r" } else { b"$KP_BIN ctl list\r" });
    kp.wait_for(20, "salida de kp ctl list", |s| {
        s.split_once("ctl list").is_some_and(|(_, out)| out.contains('1') && out.contains('2') && out.contains("shell"))
    });
    // Bloque estilo Warp: etiqueta ✔ con duración y selección con Ctrl+↑.
    kp.wait_for(20, "bloque terminado con ✔", |s| s.contains("✔ "));
    kp.send(b"[1;5A");
    kp.wait_for(20, "modo bloques", |s| s.contains("BLOQUES"));
    kp.send(ESC);
    kp.wait_for(20, "salida del modo bloques", |s| !s.contains("BLOQUES"));

    // Vista grid.
    kp.send(PREFIX);
    kp.send(b"g");
    kp.wait_for(20, "vista grid", |s| s.contains("GRID"));
    kp.send(b"\r");

    if let (Some(pid), true) = (kp.child.process_id(), cfg!(windows)) {
        let filter = format!("PID eq {pid}");
        let out = std::process::Command::new("tasklist").args(["/FI", &filter, "/FO", "CSV", "/NH"]).output().unwrap();
        println!("RAM kp (2 sesiones): {}", String::from_utf8_lossy(&out.stdout).trim());
    }
    kp.quit();
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Panel git con botones: abrir (Ctrl+Space r), clic de ratón en "Stage", commit desde la UI, cambio de tema.
#[test]
fn git_panel() {
    let dir = std::env::temp_dir().join(format!("kp-e2e-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "kp@test"]);
    git(&dir, &["config", "user.name", "kp"]);
    std::fs::write(dir.join("b.txt"), "linea-nueva\n").unwrap();

    let kp = Kp::start(&dir);
    kp.wait_for(20, "sidebar", |s| s.contains("1 shell"));
    kp.send(PREFIX);
    kp.send(b"r");
    kp.wait_for(20, "panel git", |s| s.contains("Cambios 1") && s.contains("+linea-nueva"));

    // Clic (SGR 1006, coordenadas 1-based) sobre el botón Stage (" s + " compacto o " s + Stage ").
    let (row, col) = {
        let p = kp.parser.lock().unwrap();
        let rows: Vec<String> = p.screen().rows(0, 140).collect();
        let r = rows.iter().position(|l| l.contains(" s + ")).expect("botón Stage visible");
        let c = rows[r][..rows[r].find(" s + ").unwrap()].chars().count() + 1;
        (r + 1, c + 2)
    };
    kp.send(format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m").as_bytes());
    kp.wait_for(20, "archivo en stage tras el clic", |s| s.contains("1 staged"));

    kp.send(b"c");
    kp.wait_for(20, "entrada de commit", |s| s.contains("mensaje de commit"));
    kp.send(b"commit-e2e");
    kp.send(b"\r");
    kp.wait_for(20, "repo limpio", |s| s.contains("sin cambios"));
    assert!(git(&dir, &["log", "--oneline"]).contains("commit-e2e"));

    // Tema desde la paleta.
    kp.send(PREFIX);
    kp.send(b"p");
    kp.send(b"tema dracula");
    kp.send(b"\r");
    kp.wait_for(20, "cambio de tema", |s| s.contains("tema dracula"));

    kp.send(ESC);
    kp.wait_for(20, "cierre del panel", |s| !s.contains("Cambios 0"));
    kp.quit();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Con un agente real (Claude Code) instalado: lo lanza desde la paleta, sin enviar prompts,
/// y comprueba que kp detecta su estado. `cargo test -- --ignored real_agent`
#[test]
#[ignore]
fn real_agent() {
    let dir = std::env::temp_dir().join("kp-e2e-agent");
    std::fs::create_dir_all(&dir).unwrap();
    let kp = Kp::start(&dir);
    kp.wait_for(20, "sidebar", |s| s.contains("1 shell"));
    kp.send(PREFIX);
    kp.send(b"p");
    kp.send(b"nuevo claude");
    kp.send(b"\r");
    kp.wait_for(20, "sesión claude", |s| s.contains("2 claude"));
    // Esperar a que Claude pinte su interfaz y luego a que kp clasifique su estado.
    kp.wait_for(90, "interfaz de claude", |s| s.contains("Claude") || s.contains("trust") || s.contains("❯"));
    std::thread::sleep(Duration::from_secs(3));
    kp.wait_for(60, "estado correcto de claude", |s| {
        // Diálogo de confianza = bloqueado esperando al usuario; si no, prompt libre = idle.
        // Pestaña: ▲ = bloqueado, ○ = idle.
        if s.contains("trust this folder") { s.contains("▲ 2 claude") } else { s.contains("○ 2 claude") }
    });
    println!("{}", kp.screen());
    kp.quit();
}

/// Vuelca la pantalla con sus colores a HTML (para revisar el diseño en un navegador).
fn snapshot_html(kp: &Kp, name: &str) {
    let p = kp.parser.lock().unwrap();
    let scr = p.screen();
    let (rows, cols) = scr.size();
    let css = |c: vt100::Color, default: &str| match c {
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        vt100::Color::Idx(i) => format!("var(--c{i})"),
        vt100::Color::Default => default.to_string(),
    };
    let mut html = String::from(
        "<html><head><meta charset='utf-8'></head><body style='margin:0;background:#000'><pre style='font:15px/19px Consolas,monospace;margin:0'>",
    );
    for r in 0..rows.get() {
        for c in 0..cols.get() {
            let Some(cell) = scr.cell(r, c) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (css(cell.fgcolor(), "#ddd"), css(cell.bgcolor(), "#000"));
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let ch = match cell.contents() {
                "" => " ".to_string(),
                "<" => "&lt;".into(),
                "&" => "&amp;".into(),
                s => s.to_string(),
            };
            let weight = if cell.bold() { "bold" } else { "normal" };
            html += &format!("<span style='display:inline-block;height:19px;color:{fg};background:{bg};font-weight:{weight}'>{ch}</span>");
        }
        html.push('\n');
    }
    html += "</pre></body></html>";
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("kp-snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.html")), html).unwrap();
}

/// `cargo test --test e2e snapshots -- --ignored` → target/kp-snapshots/*.html
#[test]
#[ignore]
fn snapshots() {
    let dir = std::env::temp_dir().join(format!("kp-snap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("app.rs"), "fn main() {\n    println!(\"hola\");\n}\n").unwrap();
    let kp = Kp::start(&dir);
    kp.wait_for(20, "pestaña", |s| s.contains("1 shell"));
    kp.wait_for(30, "editor", |s| s.contains("escribe un comando"));
    if cfg!(windows) {
        kp.send(b"Get-ChildItem\r");
        kp.wait_for(20, "bloque 1", |s| s.matches("✔").count() >= 1);
        kp.send(b"cmd /c exit 2\r");
        kp.wait_for(20, "bloque fallido", |s| s.contains("✘ 2"));
        kp.send(b"Write-Host 'hola desde kp' -ForegroundColor Green\r");
        kp.wait_for(20, "bloque 3", |s| s.matches("✔").count() >= 2);
        kp.send(b"Get-Ch");
        std::thread::sleep(Duration::from_millis(500));
        snapshot_html(&kp, "0-warp-editor");
        kp.send(b"\x1b[1;5A");
        kp.wait_for(20, "modo bloques", |s| s.contains("BLOQUES"));
        kp.send(b"\x1b[1;5A");
        std::thread::sleep(Duration::from_millis(400));
        snapshot_html(&kp, "0b-bloque-seleccionado");
        kp.send(ESC);
        kp.send(ESC);
    }
    kp.send(PREFIX);
    kp.send(b"n");
    kp.wait_for(20, "2 shell", |s| s.contains("2 shell"));
    std::thread::sleep(Duration::from_secs(2));
    snapshot_html(&kp, "1-sesion");
    kp.send(PREFIX);
    std::thread::sleep(Duration::from_millis(500));
    snapshot_html(&kp, "2-which-key");
    kp.send(b"p");
    kp.send(b"tema");
    std::thread::sleep(Duration::from_millis(600));
    snapshot_html(&kp, "3-paleta-temas");
    kp.send(b" light");
    std::thread::sleep(Duration::from_millis(600));
    snapshot_html(&kp, "3b-preview-light");
    kp.send(ESC);
    kp.wait_for(20, "cierre paleta", |s| !s.contains("resultados"));
    kp.send(PREFIX);
    kp.send(b"r");
    kp.wait_for(20, "panel git", |s| s.contains("Cambios"));
    std::thread::sleep(Duration::from_millis(500));
    snapshot_html(&kp, "4-git");
    kp.send(b"q");
    kp.send(PREFIX);
    kp.send(b"g");
    std::thread::sleep(Duration::from_millis(800));
    snapshot_html(&kp, "5-grid");
    kp.quit();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fondo de pantalla desde la config y cambiado en vivo desde la paleta; disposición en 80x24.
#[test]
fn wallpaper_and_narrow_layout() {
    let dir = std::env::temp_dir().join(format!("kp-e2e-wall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Degradado diagonal (PNG) y otra imagen para el cambio en vivo.
    let img = image::RgbImage::from_fn(320, 180, |x, y| image::Rgb([(x * 255 / 319) as u8, (y * 255 / 179) as u8, 160]));
    img.save(dir.join("fondo.png")).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([0, 200, 120])).save(dir.join("verde.png")).unwrap();
    let cfg = format!("wallpaper = '{}'\nwallpaper_opacity = 0.35\n", dir.join("fondo.png").display());

    let kp = Kp::launch(&dir, 24, 80, &cfg);
    kp.wait_for(20, "pestaña", |s| s.contains("1 shell"));
    // El fondo cambia solo el color de fondo de las celdas: muchas tonalidades distintas en el contenido.
    let t0 = Instant::now();
    let tones = || {
        let p = kp.parser.lock().unwrap();
        let mut seen = std::collections::HashSet::new();
        for r in 2..18 {
            for c in 5..75 {
                if let Some(vt100::Color::Rgb(a, b, d)) = p.screen().cell(r, c).map(|c| c.bgcolor()) {
                    seen.insert((a, b, d));
                }
            }
        }
        seen.len()
    };
    while tones() < 20 && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(tones() >= 20, "fondo sin pintar ({} tonos)", tones());
    assert!(!kp.screen().contains('▀'), "el fondo no debe sustituir caracteres");
    kp.wait_for(30, "editor en 80x24", |s| s.contains("escribe un comando"));
    snapshot_html(&kp, "6-fondo-80x24");

    kp.send(PREFIX);
    kp.send(b"p");
    kp.send(b"fondo elegir");
    kp.wait_for(20, "acción de fondo en la paleta", |s| s.contains("fondo de pantalla: elegir"));
    kp.send(b"\r");
    kp.wait_for(20, "entrada de ruta", |s| s.contains("FONDO"));
    // Borra la ruta actual (prellenada) y escribe la nueva, relativa al proyecto.
    for _ in 0..200 {
        kp.writer.lock().unwrap().write_all(b"\x7f").unwrap();
    }
    kp.send(b"verde.png");
    kp.send(b"\r");
    kp.wait_for(20, "fondo cambiado", |s| s.contains("fondo:") && s.contains("verde.png"));
    kp.quit();
    let _ = std::fs::remove_dir_all(&dir);
}

