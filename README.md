# kp (kodepeter)

Terminal TUI en Rust para trabajar con **varios agentes de desarrollo a la vez** (Claude Code, Codex, Gemini, Aider…).
Combina lo mejor de herdr (un PTY por agente, barra lateral con estado, API para que los agentes se orquesten)
con la UX de Warp (paleta de comandos, interfaz cuidada).

- **Ligero:** binario de ~1 MB y ~10 MB de RAM con 2 sesiones (medido en Windows, release).
- **Rápido:** render por eventos (en reposo, 1 repintado por segundo para los relojes); las ráfagas de salida se agrupan en un frame cada ~8 ms.
- Corre dentro de cualquier terminal: Windows Terminal, Warp, WezTerm, VS Code…

## Instalar

```sh
cargo install --path .
kp                # abre kp en la carpeta actual
kp C:\mi\proyecto # o en otra
```

Guía completa paso a paso (requisitos, Windows/Linux/macOS, configuración, uso en tus proyectos,
problemas frecuentes): **[docs/INSTALACION.md](docs/INSTALACION.md)**.

En Windows se usa el toolchain `stable-x86_64-pc-windows-gnu` y MinGW-w64 (WinLibs) en el PATH: `dlltool` necesita `as`.

## Uso

Barra lateral: spinner animado = `working`, `▲ blocked` (el agente espera tu decisión: permisos, y/n,
diálogos), `✔` (terminó mientras no mirabas), `○ idle`, `✕ exited`, con el tiempo en ese estado y la rama git.
Cuando un agente en segundo plano termina o se bloquea aparece un **aviso flotante** (clic para ir a él) y
suena la campana del terminal. Cada sesión tiene una **cabecera** con estado, tarea, rama, carpeta y tiempo.

Al pulsar `Ctrl+Space` aparece un **popup con todos los atajos**; no hace falta memorizarlos.

Prefijo **Ctrl+Space**, y después:

| tecla | acción |
|---|---|
| `p` / `a` | paleta: lanzar agente, agente en **git worktree aislado**, saltar a sesión, acciones |
| `r` | **revisar código**: panel git de la sesión (su worktree si es un agente aislado) |
| `n` | shell nuevo |
| `s` | mostrar/ocultar la barra lateral (más espacio para el agente) |
| `g` | vista **grid**: todos los agentes en mosaico (flechas/hjkl, Enter abre) |
| `b` | **broadcast**: el mismo prompt a todos los agentes |
| `j`/`k`, `1`-`9` | cambiar de sesión (también con clic en la barra lateral) |
| `u`/`d` | scroll del historial (también con la rueda del ratón) |
| `w` / `q` | cerrar sesión / salir (piden confirmación si hay agentes trabajando) |
| `Ctrl+Space` | envía Ctrl+Space a la sesión |

`Shift+Enter` envía una nueva línea (Meta+Enter) a Claude Code y Codex.

## Bloques y editor (estilo Warp)

En los shells (PowerShell, pwsh, cmd, bash) kp activa sola la **integración de shell**: el prompt emite marcas
OSC 133 y kp agrupa cada comando con su salida en un **bloque**:

- Franja lateral de color (verde ✔ / rojo ✘ / spinner mientras corre) y cabecera con código de salida y duración.
- **Editor fijo abajo**: multilínea (`Shift+Enter`), historial con `↑` filtrado por lo que llevas escrito,
  **sugerencia fantasma** desde el historial (`→` o `End` la acepta), `Tab` completa rutas, resaltado de sintaxis,
  `Ctrl+←/→` por palabras, `Ctrl+W/U/K` borrar, `Esc` limpiar. Muestra carpeta y rama git.
- Mientras un comando corre (o una app usa la pantalla completa: vim, less…) las teclas van directo al programa.
- `Ctrl+↑` (o clic en un bloque) entra en **modo bloques**: `↑/↓` moverse, `c` copiar comando+salida,
  `o` copiar solo la salida (al portapapeles vía OSC 52), `r` reutilizar el comando, `Esc` volver.
- El historial se guarda en la misma carpeta que la config (`history`), sin duplicados.

Las **pestañas** de arriba muestran cada sesión con su estado (clic para cambiar, `+` para lanzar). La barra
lateral de agentes es opcional: `Ctrl+Space s` o `sidebar = true`.

## Revisar código (panel git)

`Ctrl+Space r` abre un panel estilo cliente git sobre la carpeta de la sesión enfocada. Si cambias de sesión,
el panel la sigue, así que puedes revisar agente por agente, cada uno en su worktree.

- Izquierda: archivos cambiados (`XY` como `git status`: verde = staged, rojo = sin stage, `?` = nuevo).
- Derecha: diff coloreado del archivo seleccionado (staged y sin stage por separado).
- Barra de **botones clicables** (también con su tecla):

| botón | tecla | acción |
|---|---|---|
| ⟳ Actualizar | `r` | relee el estado |
| + Stage / − Unstage | `s` / `u` | archivo seleccionado |
| ++ Todo | `A` | `git add -A` |
| ✓ Commit | `c` | pide el mensaje y hace commit |
| ⎌ Descartar | `x` | descarta los cambios del archivo (**pulsar dos veces**) |
| ✎ Pedir cambio | `a` | escribe un comentario y se lo envía como prompt al agente de esa sesión |
| ⇣ Pull / ⇡ Push | `p` / `P` | en segundo plano; push pide **doble pulsación** |
| ✕ Cerrar | `q` / `Esc` | vuelve a la terminal |

`j`/`k` o clic para elegir archivo; `Espacio`/`b` o la rueda para desplazar el diff.

## Temas

Cada tema es un **entorno visual completo**: interfaz, fondo, texto y la paleta ANSI de 16 colores, que se
aplica también a lo que muestran los agentes y shells dentro de las sesiones.

Integrados: `tokyo-night` (por defecto), `catppuccin`, `dracula`, `gruvbox`, `nord`, `one-dark`, `rose-pine`,
`light`. En la paleta, al recorrer `tema: …` se **previsualiza en vivo**; Enter lo aplica. Fíjalo en la config.
Cualquier color se puede sobrescribir con `"#rrggbb"`:

```toml
theme = "catppuccin"
blocks = true        # bloques + editor estilo Warp en los shells
sidebar = false      # barra lateral de agentes visible al arrancar
transparent = false  # true: usa el fondo de tu terminal (acrílico, imagen) en vez del del tema

[colors]
accent = "#ff79c6"   # acento, botones y selección
# bg, fg, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent, ansi0 … ansi15
```

## Orquestación entre agentes: `kp ctl`

Cada sesión recibe `KP_ADDR`, `KP_TOKEN`, `KP_SESSION` (su id) y `KP_BIN` (la ruta de kp). Con eso, un agente puede
crear sub-agentes, mandarles tareas y esperar el resultado:

```sh
kp ctl list                          # id, nombre, estado, carpeta
id=$(kp ctl spawn claude --worktree) # sub-agente en su propio worktree (no te roba el foco)
kp ctl send $id "escribe tests para src/parser.rs"
kp ctl wait $id                      # bloquea hasta que deja de trabajar → idle | blocked | exited
kp ctl read $id 80                   # últimas 80 líneas de su pantalla
kp ctl close $id
```

En PowerShell: `& $env:KP_BIN ctl list`. Sin texto, `send` lee el prompt de stdin.
La API solo escucha en `127.0.0.1` y exige el token de la sesión.

Puedes pegar esto en el `CLAUDE.md` o `AGENTS.md` de tu proyecto:

> Si existe `$KP_BIN`, puedes delegar subtareas en paralelo: `kp ctl spawn claude --worktree`, `kp ctl send <id> "<tarea>"`,
> `kp ctl wait <id>` y `kp ctl read <id>`. Usa un worktree por subtarea para no pisar archivos.

## Configuración

`%APPDATA%\kodepeter\config.toml` (Windows) o `~/.config/kodepeter/config.toml`. Todo es opcional;
si defines `presets`, reemplazan a los de por defecto.

```toml
scrollback = 2000   # líneas de historial por sesión (controla la RAM)
bell = true
waiting_patterns = ["do you want", "(y/n)", "enter to confirm"]   # texto que indica "blocked"

[[presets]]
name = "shell"
cmd = "pwsh -NoLogo"
agent = false        # los shells no reciben broadcast

[[presets]]
name = "claude"
cmd = "claude"

[[presets]]
name = "claude-yolo"
cmd = "claude --dangerously-skip-permissions"
```

## Tests

```sh
cargo test                          # unitarios + PTY real + API ctl + e2e de la TUI manejada por teclas
cargo test -- --ignored real_agent  # abre Claude Code real dentro de kp y verifica que lo detecta como blocked/idle
cargo test -- --ignored snapshots   # vuelca cada vista con sus colores a target/kp-snapshots/*.html
```

## Fuera de alcance (por ahora)

- Detach/persistencia con un servidor separado (como herdr sobre SSH).
- Bloques de comandos estilo Warp (requiere integración OSC 133 con el shell).
- Splits libres: la vista grid cubre el "ver todo a la vez".
