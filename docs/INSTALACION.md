# Instalación e implementación de kp

Guía para instalar kp desde el código fuente, comprobar que funciona y usarlo en tus proyectos con agentes.

## 1. Requisitos

| | Windows | Linux / macOS |
|---|---|---|
| Sistema | Windows 10 1809 o superior (usa ConPTY) | cualquier distribución o versión reciente |
| Terminal donde corre kp | Windows Terminal (recomendado), Warp, WezTerm, terminal de VS Code | cualquiera con colores de 24 bits |
| Herramientas | Rust (toolchain GNU) + MinGW-w64, o Rust (toolchain MSVC) + Build Tools | Rust (rustup) |
| Opcional | `git` (panel de revisión y worktrees), los agentes que vayas a usar (`claude`, `codex`, `gemini`, `aider`) | igual |

> Probado en Windows 11 con el toolchain GNU. En Linux y macOS compila con las mismas dependencias,
> pero todavía no se ha probado allí.

## 2. Instalar Rust

### Windows, opción A: toolchain GNU (la usada en el desarrollo, sin Visual Studio)

En PowerShell:

```powershell
winget install --id Rustlang.Rustup -e
# Cierra y vuelve a abrir la terminal para que cargue el PATH nuevo.
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal
rustup default stable-x86_64-pc-windows-gnu

# MinGW-w64 completo: el dlltool que trae Rust necesita el ensamblador `as`.
winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e
# Vuelve a abrir la terminal otra vez.
```

Comprueba:

```powershell
cargo --version
as --version      # tiene que responder; si no, mira "Problemas frecuentes"
```

### Windows, opción B: toolchain MSVC

Instala las **Visual Studio Build Tools** con la carga de trabajo «Desarrollo de escritorio con C++»
(unos 5-7 GB). Después:

```powershell
winget install --id Rustlang.Rustup -e
rustup default stable-x86_64-pc-windows-msvc
```

Con MSVC no hace falta MinGW.

### Linux / macOS

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## 3. Compilar e instalar kp

Desde la carpeta del proyecto (clónala si viene de un remoto):

```sh
cd kodepeter
cargo install --path . --locked
```

Esto compila en modo release (unos 2 minutos la primera vez) y deja `kp` en `~/.cargo/bin`, que rustup
ya añadió al PATH.

Comprueba:

```sh
kp --version      # kp 0.1.0
kp --help         # atajos y ruta del archivo de configuración
```

Si solo quieres el binario, sin instalarlo: `cargo build --release` → `target/release/kp(.exe)`.
Es un único ejecutable de ~1 MB; puedes copiarlo a cualquier carpeta del PATH.

## 4. Verificar la instalación (opcional)

```sh
cargo test                          # 9 unitarios + 2 e2e (TUI y panel git manejados por teclas/ratón)
cargo test -- --ignored real_agent  # abre Claude Code real dentro de kp (no envía prompts, no gasta tokens)
```

## 5. Primer uso

```sh
cd C:\ruta\a\tu\proyecto
kp
```

Se abre con un shell. Todo se hace con el prefijo **Ctrl+Space** seguido de una tecla:

1. `Ctrl+Space p` → paleta → escribe `claude` → Enter: lanza Claude Code en el proyecto.
2. `Ctrl+Space p` → `claude en git worktree` → agente aislado en su propia rama y carpeta.
3. `Ctrl+Space g` → vista grid de todos los agentes; `Ctrl+Space j/k` o clic en la barra lateral para cambiar.
4. `Ctrl+Space r` → panel git para revisar lo que hizo el agente: diff, stage, commit y pedirle cambios.
5. `Ctrl+Space q` → salir (pide confirmación si algún agente está trabajando).

La lista completa de atajos y botones está en el [README](../README.md).

## 6. Configuración

Ruta del archivo (no existe hasta que lo crees):

- Windows: `%APPDATA%\kodepeter\config.toml`
- Linux/macOS: `~/.config/kodepeter/config.toml`

Ejemplo completo:

```toml
theme = "tokyo-night"   # tokyo-night | catppuccin | dracula | gruvbox | nord | one-dark | rose-pine | light
transparent = false     # true: respeta el fondo de tu terminal (acrílico/imagen)
scrollback = 2000       # líneas de historial por sesión (más líneas = más RAM)
blocks = true           # bloques + editor estilo Warp en los shells (integración automática)
sidebar = false         # barra lateral de agentes al arrancar (Ctrl+Space s la alterna)
bell = true             # pitido cuando un agente en segundo plano termina o se bloquea

[colors]                # opcional: sobrescribe colores del tema con "#rrggbb"
accent = "#ff79c6"

# Si defines presets, sustituyen a los de por defecto: incluye también el shell.
[[presets]]
name = "shell"
cmd = "pwsh -NoLogo"
agent = false

[[presets]]
name = "claude"
cmd = "claude"

[[presets]]
name = "codex"
cmd = "codex"
```

Si el archivo tiene un error, kp arranca con los valores por defecto y lo muestra en la barra de estado.

## 7. Implementarlo en tus proyectos

### Que tus agentes puedan orquestarse

Cada sesión de kp recibe `KP_BIN`, `KP_ADDR`, `KP_TOKEN` y `KP_SESSION`. Añade esto al `CLAUDE.md` o
`AGENTS.md` del proyecto para que el agente sepa que puede delegar:

```markdown
## Orquestación con kp
Si existe la variable `KP_BIN`, estás dentro de kp y puedes repartir subtareas en paralelo:
- `& $env:KP_BIN ctl spawn claude --worktree` (PowerShell) o `$KP_BIN ctl spawn claude --worktree` (sh): crea un sub-agente aislado e imprime su id.
- `kp ctl send <id> "<tarea>"`, `kp ctl wait <id>`, `kp ctl read <id> 80`: asignar, esperar y leer el resultado.
- Usa un worktree por subtarea para no pisar archivos; revisa y fusiona su rama al terminar.
```

### Perfil de Windows Terminal para abrir kp directamente

En Windows Terminal: Configuración → «Abrir archivo JSON» y añade dentro de `profiles.list`:

```json
{
    "name": "kp",
    "commandline": "%USERPROFILE%\\.cargo\\bin\\kp.exe",
    "startingDirectory": "%USERPROFILE%\\Desktop",
    "icon": "⚡"
}
```

Si quieres que abra directamente un proyecto, pásale la ruta: `"commandline": "...\\kp.exe C:\\ruta\\proyecto"`.

### Worktrees de los agentes

Los agentes lanzados «en git worktree» crean una carpeta hermana del repo (`<repo>-<agente>-<marca>`)
y una rama `kp/<agente>-<marca>`. Cuando el trabajo esté revisado y en commit:

```sh
git merge kp/claude-1763300000        # desde tu rama principal
git worktree remove ../mi-repo-claude-1763300000
git branch -d kp/claude-1763300000
```

## 8. Actualizar y desinstalar

```sh
git pull                                 # si viene de un remoto
cargo install --path . --locked --force  # recompila y reemplaza el binario
```

Desinstalar:

```sh
cargo uninstall kp
# Opcional: borrar la configuración (%APPDATA%\kodepeter o ~/.config/kodepeter)
# Opcional (Windows): winget uninstall BrechtSanders.WinLibs.POSIX.UCRT ; winget uninstall Rustlang.Rustup
```

## 9. Problemas frecuentes

| Síntoma | Causa y solución |
|---|---|
| `cargo: command not found` / `kp` no se reconoce | El PATH no se recargó: cierra y abre la terminal. Comprueba que `~/.cargo/bin` está en el PATH. |
| `error calling dlltool 'dlltool.exe': program not found` o `dlltool ... CreateProcess` | Toolchain GNU sin binutils completos: instala WinLibs (paso 2A) y reabre la terminal. `as --version` debe funcionar. |
| `'codex' no está en el PATH` al lanzar un agente | El agente no está instalado o su carpeta no está en el PATH (`npm i -g @openai/codex`, etc.). kp resuelve `.exe`/`.cmd` como cmd.exe. |
| Ctrl+Space no hace nada | Otra aplicación (cambio de idioma o IME del sistema, atajos de tu terminal) captura la combinación. Libérala ahí; el prefijo se define en `src/keys.rs` (`is_prefix`). |
| Un shell no muestra bloques ni editor | Solo PowerShell, pwsh, cmd y bash tienen integración automática. Otros shells funcionan como terminal clásica. Si tu perfil redefine `prompt` de forma diferida, revisa que se ejecute antes del hook de kp. |
| `c`/`o` en modo bloques no copia | El portapapeles va por OSC 52: Windows Terminal y WezTerm lo soportan; otras terminales pueden bloquearlo. |
| kp se ve sin colores | Tienes definida la variable `NO_COLOR` (kp la respeta). Bórrala en ese entorno: `Remove-Item Env:NO_COLOR` o `unset NO_COLOR`. |
| No puedo seleccionar texto con el ratón | kp captura el ratón para clics y rueda. En Windows Terminal, mantén **Shift** al arrastrar para seleccionar. |
| Un agente aparece `idle` cuando está esperando una respuesta | Su diálogo no coincide con los patrones de «blocked». Añade el texto que muestra a `waiting_patterns` en la config (si defines la lista reemplaza a la de fábrica, que está en `src/config.rs`: cópiala y amplíala). |
| Push desde el panel falla pidiendo credenciales | kp ejecuta git sin terminal interactiva. Configura un gestor de credenciales o SSH y haz un `git push` manual una vez. |
