//! Integración de shell (estilo Warp): el prompt emite marcas OSC 133 (inicio de prompt, fin de comando
//! con su código) y la carpeta actual, para que kp agrupe cada comando y su salida en un bloque.
use std::path::Path;

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Envuelve el prompt existente (respeta el del perfil: oh-my-posh, starship...). Las marcas se escriben
/// con [Console]::Write para que PSReadLine no las cuente como ancho del prompt.
const PS_HOOK: &str = r#"
$global:__kpPrompt = $function:prompt
function global:prompt {
  $ok = $?
  $c = if ($ok) { 0 } elseif ($global:LASTEXITCODE) { $global:LASTEXITCODE } else { 1 }
  $e = [char]27; $b = [char]7
  $p = $executionContext.SessionState.Path.CurrentLocation.ProviderPath
  [Console]::Write("$e]133;D;$c$b$e]9;9;$p$b$e]133;A$b")
  & $global:__kpPrompt
}
"#;

/// Argumentos y variables de entorno extra para que `prog` emita las marcas de bloque.
pub fn integration(prog: &str) -> (Vec<String>, Vec<(String, String)>) {
    let stem = Path::new(prog).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    match stem.as_str() {
        "powershell" | "pwsh" => {
            let utf16: Vec<u8> = PS_HOOK.encode_utf16().flat_map(u16::to_le_bytes).collect();
            (vec!["-NoExit".into(), "-EncodedCommand".into(), base64(&utf16)], vec![])
        }
        // cmd.exe no expone el código de salida en el prompt: bloques sin ✔/✘, pero con duración.
        "cmd" => (vec![], vec![("PROMPT".into(), r"$e]133;D$e\$e]133;A$e\$P$G".into())]),
        "bash" => (
            vec![],
            vec![("PROMPT_COMMAND".into(), r#"printf '\e]133;D;%s\a\e]7;file://%s%s\a\e]133;A\a' "$?" "$HOSTNAME" "$PWD""#.into())],
        ),
        _ => (vec![], vec![]),
    }
}

/// Ruta de un `file://host/ruta` (OSC 7). En Windows quita la `/` previa a la letra de unidad.
pub fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let path = path.replace("%20", " ");
    let b = path.as_bytes();
    Some(if b.len() > 2 && b[2] == b':' { path[1..].to_string() } else { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"hola kp!"), "aG9sYSBrcCE=");
    }

    #[test]
    fn picks_integration() {
        assert_eq!(integration(r"C:\Windows\powershell.exe").0[0], "-NoExit");
        assert_eq!(integration("pwsh").0.len(), 3);
        assert_eq!(integration("cmd.exe").1[0].0, "PROMPT");
        assert!(integration("claude").0.is_empty() && integration("claude").1.is_empty());
        assert_eq!(file_url_path("file://host/C:/Users/x%20y").as_deref(), Some("C:/Users/x y"));
        assert_eq!(file_url_path("file://h/home/u").as_deref(), Some("/home/u"));
    }
}
