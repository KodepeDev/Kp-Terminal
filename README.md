# kp (kodepeter)

Terminal TUI en Rust para trabajar con **varios agentes de desarrollo a la vez** (Claude Code, Codex, Gemini, Aider…).
Combina lo mejor de herdr (un PTY por agente, barra lateral con estado, API para que los agentes se orquesten)
con la UX de Warp (paleta de comandos, interfaz cuidada).

- **Ligero:** binario de ~1 MB y ~10 MB de RAM con 2 sesiones (medido en Windows, release).
- **Rápido:** render por eventos (0 % CPU en reposo); las ráfagas de salida se agrupan en un frame cada ~8 ms.
- Corre dentro de cualquier terminal: Windows Terminal, Warp, WezTerm, VS Code…

## Instalar

```sh
cargo install --path .
kp                # abre kp en la carpeta actual
kp C:\mi\proyecto # o en otra
```

En Windows se usa el toolchain `stable-x86_64-pc-windows-gnu` y MinGW-w64 (WinLibs) en el PATH: `dlltool` necesita `as`.

## Uso

Barra lateral: `● working`, `▲ blocked` (el agente espera tu decisión: permisos, y/n, diálogos),
`✔` (terminó mientras no mirabas), `○ idle`, `✕ exited`. Cuando un agente en segundo plano termina o se
bloquea, suena la campana del terminal.

Prefijo **Ctrl+Space**, y después:

| tecla | acción |
|---|---|
| `p` / `a` | paleta: lanzar agente, agente en **git worktree aislado**, saltar a sesión, acciones |
| `r` | **revisar código**: panel git de la sesión (su worktree si es un agente aislado) |
| `n` | shell nuevo |
| `g` | vista **grid**: todos los agentes en mosaico (flechas/hjkl, Enter abre) |
| `b` | **broadcast**: el mismo prompt a todos los agentes |
| `j`/`k`, `1`-`9` | cambiar de sesión (también con clic en la barra lateral) |
| `u`/`d` | scroll del historial (también con la rueda del ratón) |
| `w` / `q` | cerrar sesión / salir (piden confirmación si hay agentes trabajando) |
| `Ctrl+Space` | envía Ctrl+Space a la sesión |

`Shift+Enter` envía una nueva línea (Meta+Enter) a Claude Code y Codex.

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

Integrados: `tokyo-night` (por defecto), `catppuccin`, `dracula`, `gruvbox`, `nord`, `light`.
Pruébalos en vivo desde la paleta (`tema: …`) y fija el elegido en la config. Cualquier color se puede
sobrescribir con `"#rrggbb"`:

```toml
theme = "catppuccin"

[colors]
accent = "#ff79c6"   # acento, botones y selección
# dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent
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
```

## Fuera de alcance (por ahora)

- Detach/persistencia con un servidor separado (como herdr sobre SSH).
- Bloques de comandos estilo Warp (requiere integración OSC 133 con el shell).
- Splits libres: la vista grid cubre el "ver todo a la vez".
